use std::ffi::{c_void, CStr};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::{self, addr_of};

use windows_sys::Win32::Foundation::{
    LocalFree, ERROR_ALREADY_EXISTS, ERROR_INSUFFICIENT_BUFFER, HANDLE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidA, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetSecurityInfo, SetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetAce, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
    GetSecurityDescriptorOwner, GetTokenInformation, GetLengthSid, IsValidSid, TokenUser,
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION,
    INHERITED_ACE, INHERIT_ONLY_ACE, OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSID, SECURITY_ATTRIBUTES, SE_DACL_PROTECTED,
    TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateDirectoryW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ALL_ACCESS,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
};
use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::insecure_directory;

const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // NOTE: The Windows API transfers a LocalFree allocation here; no safe Rust owner can release this native pointer.
        unsafe { LocalFree(self.0) };
    }
}

struct AccountSid(Vec<usize>);

impl AccountSid {
    fn current() -> io::Result<Self> {
        let mut handle = ptr::null_mut();
        // NOTE: The process pseudo-handle and writable output slot meet OpenProcessToken's contract; no safe API opens this token.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if handle.is_null() {
            return Err(invalid_identity());
        }
        // NOTE: OpenProcessToken succeeded and transferred one owned handle; OwnedHandle closes it because no safe token API exists.
        let token = unsafe { OwnedHandle::from_raw_handle(handle) };
        let mut size = 0;
        // NOTE: Null/zero is the documented size-query form and size is writable; no safe API exposes TOKEN_USER sizing.
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
        // NOTE: The aligned buffer covers the queried size and stays writable; no safe API fills TOKEN_USER from this token.
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
        let initialized = size as usize;
        let allocated = buffer.len() * size_of::<usize>();
        if initialized < size_of::<TOKEN_USER>() || initialized > allocated {
            return Err(invalid_identity());
        }
        // NOTE: GetTokenInformation succeeded and initialized TOKEN_USER within the checked buffer; Rust cannot model its embedded SID pointer.
        let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let start = buffer.as_ptr() as usize;
        let Some(end) = start.checked_add(initialized) else {
            return Err(invalid_identity());
        };
        let sid_address = sid as usize;
        let Some(sid_header_end) = sid_address.checked_add(8) else {
            return Err(invalid_identity());
        };
        if sid.is_null() || sid_address < start || sid_header_end > end {
            return Err(invalid_identity());
        }
        // NOTE: The checked range contains the full SID header; GetLengthSid reads its bounded subauthority count, which Rust cannot inspect safely through PSID.
        let sid_length = unsafe { GetLengthSid(sid) as usize };
        if sid_length < 8 || sid_length > end - sid_address {
            return Err(invalid_identity());
        }
        // NOTE: GetLengthSid proved the full SID lies in buffer; IsValidSid checks that bounded native structure without a safe Rust equivalent.
        if unsafe { IsValidSid(sid) } == 0 {
            return Err(invalid_identity());
        }
        Ok(Self(buffer))
    }

    fn sid(&self) -> PSID {
        // NOTE: current() validated TOKEN_USER and the SID bounds; self keeps their buffer alive, and no safe API exposes PSID.
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    fn security_descriptor(&self) -> io::Result<LocalAllocation> {
        let mut ascii_sid = ptr::null_mut();
        // NOTE: The validated SID buffer and writable output slot live through conversion; only the native API converts this SID.
        if unsafe { ConvertSidToStringSidA(self.sid(), &mut ascii_sid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if ascii_sid.is_null() {
            return Err(invalid_identity());
        }
        let ascii_sid = LocalAllocation(ascii_sid.cast());
        // NOTE: ConvertSidToStringSidA returned a non-null NUL-terminated buffer owned by ascii_sid; CStr is the safe borrowed view.
        let sid = unsafe { CStr::from_ptr(ascii_sid.0.cast()) }.to_string_lossy();
        let descriptor = format!("O:{sid}D:P(A;OICI;FA;;;{sid})");
        let utf16: Vec<u16> = descriptor.encode_utf16().chain([0]).collect();
        let mut result = ptr::null_mut();
        // NOTE: The UTF-16 descriptor is NUL-terminated and both buffers live through the native call; no safe API builds this ACL descriptor.
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
        if result.is_null() {
            return Err(invalid_identity());
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
    // NOTE: The NUL-terminated path and protected descriptor live through CreateDirectoryW; std create_dir cannot supply this ACL at creation.
    if unsafe { CreateDirectoryW(name.as_ptr(), &attributes) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
            return Err(error);
        }
    }
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let information = file_information(&directory)?;
    if information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        || information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(insecure_directory(path));
    }
    if !is_private(directory.as_raw_handle(), &account)? {
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
    const SYNCHRONIZE: u32 = 0x0010_0000;
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
        {
            return Err(io::Error::last_os_error());
        }
        if dacl_present == 0 || dacl.is_null() || owner.is_null() {
            return Err(unsafe_file());
        }
        (owner, dacl, descriptor)
    };

    let (temp_owner, _, temp_descriptor) = query_file_security(temp_file.as_raw_handle())?;
    let owner_matches = sid_matches(temp_owner, owner);
    let access = FILE_READ_ATTRIBUTES
        | READ_CONTROL
        | SYNCHRONIZE
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
    // NOTE: The opened handle matches the empty temp file and both descriptors keep owner/DACL pointers live; Win32 has no safe ACL setter.
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
    // NOTE: The opened handle stays live and output slots are writable; the returned descriptor owns owner/DACL pointers, with no safe handle-DACL API.
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
    if owner.is_null() || descriptor.is_null() {
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
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| unsafe_file())?;
    let information = file_information(&file).map_err(|_| unsafe_file())?;
    if information.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || information.nNumberOfLinks != 1
        || !is_private_file(file.as_raw_handle(), &AccountSid::current()?)?
    {
        return Err(unsafe_file());
    }
    Ok(file)
}

fn is_private(handle: HANDLE, account: &AccountSid) -> io::Result<bool> {
    let (owner, dacl, descriptor) = query_file_security(handle)?;
    let mut control = 0;
    let mut revision = 0;
    // NOTE: The queried descriptor stays allocated and control/revision are writable; std has no handle-DACL query.
    if unsafe { GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(control & SE_DACL_PROTECTED != 0
        && sid_matches(owner, account.sid())
        && only_user_access(dacl, account))
}

fn is_private_file(handle: HANDLE, account: &AccountSid) -> io::Result<bool> {
    let (owner, dacl, _descriptor) = query_file_security(handle)?;
    if !sid_matches(owner, account.sid()) {
        return Ok(false);
    }
    let Some(ace) = single_ace(dacl) else {
        return Ok(false);
    };
    let allowed_flags = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE | INHERITED_ACE;
    Ok(allowed_ace_matches(dacl, ace, allowed_flags, false, account))
}

fn unsafe_file() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "stored credential file is not a private regular file",
    )
}

fn only_user_access(dacl: *const ACL, account: &AccountSid) -> bool {
    let Some(ace) = single_ace(dacl) else {
        return false;
    };
    let inheritable = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;
    allowed_ace_matches(dacl, ace, inheritable, true, account)
}

fn single_ace(dacl: *const ACL) -> Option<*mut c_void> {
    if dacl.is_null() {
        return None;
    }
    // NOTE: dacl is owned by a live security descriptor; Windows returned this ACL header, which Rust cannot inspect safely.
    let (ace_count, acl_size) = unsafe { ((*dacl).AceCount, (*dacl).AclSize) };
    if ace_count != 1 || usize::from(acl_size) < size_of::<ACL>() {
        return None;
    }
    let mut ace = ptr::null_mut();
    // NOTE: dacl remains live and ace is a writable output slot; GetAce is the native bounded ACL accessor.
    if unsafe { GetAce(dacl, 0, &mut ace) } == 0 || ace.is_null() {
        return None;
    }
    Some(ace)
}

fn allowed_ace_matches(
    dacl: *const ACL,
    ace: *mut c_void,
    allowed_flags: u32,
    exact_flags: bool,
    account: &AccountSid,
) -> bool {
    const SID_OFFSET: usize = size_of::<ACE_HEADER>() + size_of::<u32>();
    const SID_HEADER_SIZE: usize = 8;

    // NOTE: GetAce supplied an ACE inside dacl; ACL/AceSize bounds are checked before SID access, and no safe API parses native ACEs.
    unsafe {
        let acl_size = usize::from((*dacl).AclSize);
        let Some(ace_offset) = (ace as usize).checked_sub(dacl as usize) else {
            return false;
        };
        if ace_offset < size_of::<ACL>()
            || ace_offset
                .checked_add(size_of::<ACE_HEADER>())
                .map_or(true, |end| end > acl_size)
        {
            return false;
        }
        let header = ace.cast::<ACE_HEADER>().read();
        let ace_size = usize::from(header.AceSize);
        let Some(ace_end) = ace_offset.checked_add(ace_size) else {
            return false;
        };
        if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE
            || ace_size < SID_OFFSET + SID_HEADER_SIZE
            || ace_end > acl_size
        {
            return false;
        }
        let flags = u32::from(header.AceFlags);
        if flags & !allowed_flags != 0
            || flags & INHERIT_ONLY_ACE != 0
            || (exact_flags && flags != allowed_flags)
        {
            return false;
        }
        let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
        let sid = addr_of!((*allowed).SidStart).cast_mut().cast();
        let sid_available = ace_size - SID_OFFSET;
        let sid_length = GetLengthSid(sid) as usize;
        sid_length >= SID_HEADER_SIZE
            && sid_length <= sid_available
            && IsValidSid(sid) != 0
            && (*allowed).Mask == FILE_ALL_ACCESS
            && EqualSid(sid, account.sid()) != 0
    }
}

fn sid_matches(left: PSID, right: PSID) -> bool {
    if left.is_null() || right.is_null() {
        return false;
    }
    // NOTE: Both SIDs come from live, validated token/security-descriptor buffers; Rust has no safe native SID comparator.
    unsafe { EqualSid(left, right) != 0 }
}

fn invalid_identity() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "cannot inspect current user identity",
    )
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
