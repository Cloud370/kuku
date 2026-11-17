#[cfg(unix)]
use std::io;

#[cfg(unix)]
use tokio::process::{Child, Command};

#[cfg(unix)]
const SIGNAL_KILL: i32 = 9;
#[cfg(unix)]
const NO_SUCH_PROCESS: i32 = 3;

#[cfg(unix)]
pub(super) struct ProcessTree {
    group: Option<i32>,
}

#[cfg(unix)]
impl ProcessTree {
    pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, Self)> {
        command.process_group(0).kill_on_drop(true);
        let child = command.spawn()?;
        let group = child.id().map(|id| id as i32);
        Ok((child, Self { group }))
    }

    pub(super) fn terminate(&mut self) -> io::Result<()> {
        let Some(group) = self.group.take() else {
            return Ok(());
        };
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // NOTE: The positive child id names our dedicated process group; negative pid targets only that group. Rust has no safe group-signal API.
        let result = unsafe { kill(-group, SIGNAL_KILL) };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(NO_SUCH_PROCESS) {
            Ok(())
        } else {
            self.group = Some(group);
            Err(error)
        }
    }
}

#[cfg(unix)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::ptr;

    use tokio::process::{Child, Command};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenThread, ResumeThread, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
    };

    pub(crate) struct ProcessTree {
        job: Option<OwnedHandle>,
    }

    impl ProcessTree {
        pub(crate) fn spawn(command: &mut Command) -> io::Result<(Child, Self)> {
            let job = create_job()?;
            command.creation_flags(CREATE_SUSPENDED).kill_on_drop(true);
            let mut child = command.spawn()?;
            let result = assign_and_resume(&job, &child);
            if let Err(error) = result {
                let _ = child.start_kill();
                return Err(error);
            }
            Ok((child, Self { job: Some(job) }))
        }

        pub(crate) fn terminate(&mut self) -> io::Result<()> {
            self.job.take();
            Ok(())
        }
    }

    fn create_job() -> io::Result<OwnedHandle> {
        // NOTE: Null security/name pointers request an unnamed job with default security; the returned non-null handle is newly owned.
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // NOTE: CreateJobObjectW transferred a valid unique handle; OwnedHandle closes it on every exit path.
        let job = unsafe { OwnedHandle::from_raw_handle(raw) };
        // NOTE: This Win32 C structure permits all-zero initialization before its documented limit flag is set.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // NOTE: The job handle and correctly sized limits remain valid throughout the call; Win32 exposes no safe wrapper.
        let result = unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                ptr::from_ref(&limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if result == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn assign_and_resume(job: &OwnedHandle, child: &Child) -> io::Result<()> {
        let process = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("command has no process handle"))?;
        // NOTE: Both handles are live and the child is suspended, so no descendant can escape before job assignment.
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), process) } == 0 {
            return Err(io::Error::last_os_error());
        }
        resume_primary_thread(
            child
                .id()
                .ok_or_else(|| io::Error::other("command has no process id"))?,
        )
    }

    fn resume_primary_thread(process_id: u32) -> io::Result<()> {
        // NOTE: TH32CS_SNAPTHREAD ignores the process argument and returns a newly owned snapshot handle or INVALID_HANDLE_VALUE.
        let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // NOTE: The successful snapshot call transferred this valid unique handle.
        let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
        // NOTE: THREADENTRY32 is a zero-initializable Win32 C structure; dwSize is required before enumeration.
        let mut entry: THREADENTRY32 = unsafe { zeroed() };
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        // NOTE: The snapshot and correctly sized writable entry remain live throughout enumeration.
        let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
        while found != 0 {
            if entry.th32OwnerProcessID == process_id {
                return resume_thread(entry.th32ThreadID);
            }
            // NOTE: The same valid snapshot and entry are retained until enumeration finishes.
            found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
        }
        Err(io::Error::other(
            "suspended command primary thread not found",
        ))
    }

    fn resume_thread(thread_id: u32) -> io::Result<()> {
        // NOTE: The suspended child's enumerated thread id is opened with only resume rights and no handle inheritance.
        let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // NOTE: OpenThread transferred a valid unique handle that must be closed even when resuming fails.
        let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
        // NOTE: This live handle has resume rights, and CREATE_SUSPENDED gave the primary thread one suspension count.
        if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(windows)]
pub(super) use windows::ProcessTree;
