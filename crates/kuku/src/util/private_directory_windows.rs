use std::ffi::{c_void, CStr};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::{self, addr_of};

use windows_sys::Win32::Foundation::{
    LocalFree, ERROR_ALREADY_EXISTS, ERROR_INSUFFICIENT_BUFFER, HANDLE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidA, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, GetSecurityInfo, SetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetAce, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
    GetSecurityDescriptorOwner, GetTokenInformation, TokenUser, ACCESS_ALLOWED_ACE, ACL,
    CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, INHERITED_ACE, INHERIT_ONLY_ACE,
    OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSID,
    SECURITY_ATTRIBUTES, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateDirectoryW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ALL_ACCESS,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
};
use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::insecure_directory;

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // NOTE: Windows returned this allocation for LocalFree. Its associated data is no longer used.
        unsafe { LocalFree(self.0) };
    }
}

struct AccountSid(Vec<usize>);

impl AccountSid {
    fn current() -> io::Result<Self> {
        let mut handle = ptr::null_mut();
        // NOTE: The process pseudo-handle and writable output slot meet OpenProcessToken's contract.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // NOTE: OpenProcessToken succeeded and transferred an owned token handle.
        let token = unsafe { OwnedHandle::from_raw_handle(handle) };
        let mut size = 0;
        // NOTE: A null buffer and zero length tell Windows to return the required allocation size.
        let success = unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                ptr::null_mut(),
                0,
                &mut size,
            )
        };
        if success == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
                return Err(error);
            }
        }
        let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        // NOTE: The buffer is aligned for TOKEN_USER and has at least the requested byte length.
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(buffer))
    }

    fn sid(&self) -> PSID {
        // NOTE: GetTokenInformation initialized TOKEN_USER and its SID in this live buffer.
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    fn security_descriptor(&self) -> io::Result<LocalAllocation> {
        let mut ascii_sid = ptr::null_mut();
        // NOTE: The SID lives in AccountSid's token buffer and the output is writable.
        if unsafe { ConvertSidToStringSidA(self.sid(), &mut ascii_sid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let ascii_sid = LocalAllocation(ascii_sid.cast());
        // NOTE: ConvertSidToStringSidA returns a NUL-terminated ASCII SID in this allocation.
        let sid = unsafe { CStr::from_ptr(ascii_sid.0.cast()) }.to_string_lossy();
        let descriptor = format!("O:{sid}D:P(A;OICI;FA;;;{sid})");
        let utf16: Vec<u16> = descriptor.encode_utf16().chain([0]).collect();
        let mut result = ptr::null_mut();
        // NOTE: The descriptor is NUL-terminated and the result slot remains valid during the call.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                utf16.as_ptr(),
                SDDL_REVISION_1,
                &mut result,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(LocalAllocation(result))
    }
}

pub(super) fn private_directory(path: &Path) -> io::Result<()> {
    let account = AccountSid::current()?;
    let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
    if name.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "directory path contains NUL",
        ));
    }
    name.push(0);
    let descriptor = account.security_descriptor()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    // NOTE: The path and explicit, protected security descriptor live through CreateDirectoryW.
    if unsafe { CreateDirectoryW(name.as_ptr(), &attributes) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
            return Err(error);
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(insecure_directory(path));
    }
    if !is_private(&name, &account)? {
        return Err(insecure_directory(path));
    }
    Ok(())
}

pub(super) fn set_config_temp_security(
    source_path: Option<&Path>,
    temp_path: &Path,
    temp_file: &File,
) -> io::Result<()> {
    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const READ_CONTROL: u32 = 0x0002_0000;
    const WRITE_DAC: u32 = 0x0004_0000;
    const WRITE_OWNER: u32 = 0x0008_0000;

    let source_file = source_path.map(open_config_file).transpose()?;
    let (owner, dacl, descriptor) = if let Some(source) = &source_file {
        query_file_security(source.as_raw_handle())?
    } else {
        let account = AccountSid::current()?;
        let descriptor = account.security_descriptor()?;
        let mut owner = ptr::null_mut();
        let mut owner_defaulted = 0;
        // NOTE: The descriptor allocation remains live, the owner slot is writable, and no safe API exposes this SID.
        if unsafe {
            GetSecurityDescriptorOwner(descriptor.0, &mut owner, &mut owner_defaulted)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut dacl_present = 0;
        let mut dacl_defaulted = 0;
        let mut dacl = ptr::null_mut();
        // NOTE: The same live descriptor owns the returned DACL; native descriptor access has no safe wrapper.
        if unsafe {
            GetSecurityDescriptorDacl(
                descriptor.0,
                &mut dacl_present,
                &mut dacl,
                &mut dacl_defaulted,
            )
        } == 0
            || dacl_present == 0
            || dacl.is_null()
        {
            return Err(io::Error::last_os_error());
        }
        (owner, dacl, descriptor)
    };

    let (temp_owner, _, temp_descriptor) = query_file_security(temp_file.as_raw_handle())?;
    let owner_matches = unsafe { EqualSid(temp_owner, owner) != 0 };
    let access = FILE_READ_ATTRIBUTES
        | READ_CONTROL
        | WRITE_DAC
        | if owner_matches { 0 } else { WRITE_OWNER };
    let destination = OpenOptions::new()
        .access_mode(access)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(temp_path)?;
    let opened_info = file_information(&destination)?;
    let original_info = file_information(temp_file)?;
    if opened_info.dwVolumeSerialNumber != original_info.dwVolumeSerialNumber
        || opened_info.nFileIndexHigh != original_info.nFileIndexHigh
        || opened_info.nFileIndexLow != original_info.nFileIndexLow
        || opened_info.dwFileAttributes
            & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
            != 0
        || opened_info.nNumberOfLinks != 1
    {
        return Err(unsafe_file());
    }

    let mut security = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
    let owner = if owner_matches {
        ptr::null_mut()
    } else {
        security |= OWNER_SECURITY_INFORMATION;
        owner
    };
    // NOTE: The destination is the verified temp-file handle; owner/DACL pointers remain in live descriptors, and Win32 has no safe ACL setter.
    let error = unsafe {
        SetSecurityInfo(
            destination.as_raw_handle(),
            SE_FILE_OBJECT,
            security,
            owner,
            ptr::null_mut(),
            dacl,
            ptr::null_mut(),
        )
    };
    let _ = (descriptor, temp_descriptor);
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    Ok(())
}

fn open_config_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let information = file_information(&file)?;
    if information.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
        != 0
    {
        return Err(unsafe_file());
    }
    Ok(file)
}

fn query_file_security(handle: HANDLE) -> io::Result<(PSID, *mut ACL, LocalAllocation)> {
    let mut owner = ptr::null_mut();
    let mut dacl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // NOTE: The caller keeps the opened handle live and all output slots writable; safe Rust cannot query this handle's DACL.
    let error = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    if owner.is_null() {
        return Err(unsafe_file());
    }
    Ok((owner, dacl, LocalAllocation(descriptor)))
}

fn file_information(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // NOTE: File owns a live handle and information is writable; path metadata cannot establish reparse status for this handle.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(information)
}

pub(super) fn open_private_key(path: &Path) -> io::Result<File> {
    let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
    if name.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "credential file path contains NUL",
        ));
    }
    name.push(0);
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| unsafe_file())?;
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(unsafe_file());
    }
    if information.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || information.nNumberOfLinks != 1
        || !is_private_file(file.as_raw_handle(), &AccountSid::current()?)?
    {
        return Err(unsafe_file());
    }
    Ok(file)
}

fn is_private(name: &[u16], account: &AccountSid) -> io::Result<bool> {
    let mut owner = ptr::null_mut();
    let mut dacl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // NOTE: The path is NUL-terminated and the output slots receive pointers into one allocation.
    let error = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    let descriptor = LocalAllocation(descriptor);
    let mut control = 0;
    let mut revision = 0;
    // NOTE: Both descriptors and the queried owner SID live through these native checks.
    if unsafe { GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(control & SE_DACL_PROTECTED != 0
        && !owner.is_null()
        && unsafe { EqualSid(owner, account.sid()) } != 0
        && only_user_access(dacl, account))
}

fn is_private_file(handle: HANDLE, account: &AccountSid) -> io::Result<bool> {
    let mut owner = ptr::null_mut();
    let mut dacl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // NOTE: Security is queried from the opened handle, so path replacement cannot change the object checked.
    let error = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    let _descriptor = LocalAllocation(descriptor);
    let mut ace = ptr::null_mut();
    if owner.is_null()
        || unsafe { EqualSid(owner, account.sid()) == 0 }
        || dacl.is_null()
        || unsafe { (*dacl).AceCount != 1 || GetAce(dacl, 0, &mut ace) == 0 }
    {
        return Ok(false);
    }
    // NOTE: GetAce returned an aligned ACE header in the live security descriptor.
    Ok(unsafe {
        let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
        let header = ace
            .cast::<windows_sys::Win32::Security::ACE_HEADER>()
            .read();
        let flags = u32::from(header.AceFlags);
        let allowed_flags = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE | INHERITED_ACE;
        u32::from(header.AceType) == ACCESS_ALLOWED_ACE_TYPE
            && usize::from(header.AceSize) >= size_of::<ACCESS_ALLOWED_ACE>()
            && flags & !allowed_flags == 0
            && flags & INHERIT_ONLY_ACE == 0
            && (*ace).Mask == FILE_ALL_ACCESS
            && EqualSid(addr_of!((*ace).SidStart).cast_mut().cast(), account.sid()) != 0
    })
}

fn unsafe_file() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "stored credential file is not a private regular file",
    )
}

fn only_user_access(dacl: *const ACL, account: &AccountSid) -> bool {
    let mut ace = ptr::null_mut();
    // NOTE: GetNamedSecurityInfoW returned a native ACL in a live allocation.
    if dacl.is_null() || unsafe { (*dacl).AceCount != 1 || GetAce(dacl, 0, &mut ace) == 0 } {
        return false;
    }
    // NOTE: GetAce returned an aligned ACE header. Its type and size establish the allow form.
    unsafe {
        let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
        let header = ace
            .cast::<windows_sys::Win32::Security::ACE_HEADER>()
            .read();
        if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE
            || usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>()
        {
            return false;
        }
        u32::from(header.AceFlags) == (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE)
            && (*ace).Mask == FILE_ALL_ACCESS
            && EqualSid(addr_of!((*ace).SidStart).cast_mut().cast(), account.sid()) != 0
    }
}

#[cfg(test)]
mod config_temp_tests {
    use std::os::windows::io::AsRawHandle;

    use super::{is_private_file, set_config_temp_security, AccountSid};

    #[test]
    fn config_temp_acl_is_private_and_copied_from_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let source = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
        set_config_temp_security(None, source.path(), source.as_file()).unwrap();

        let copied = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
        set_config_temp_security(
            Some(source.path()),
            copied.path(),
            copied.as_file(),
        )
        .unwrap();

        let account = AccountSid::current().unwrap();
        assert!(is_private_file(source.as_file().as_raw_handle(), &account).unwrap());
        assert!(is_private_file(copied.as_file().as_raw_handle(), &account).unwrap());
    }
}
