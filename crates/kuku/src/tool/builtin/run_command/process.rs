#[cfg(unix)]
use std::io;
#[cfg(target_os = "macos")]
use std::mem::size_of;
#[cfg(unix)]
use std::process::ExitStatus;

#[cfg(unix)]
use tokio::process::{Child, Command};
#[cfg(target_os = "macos")]
use tokio::signal::unix::{Signal, SignalKind, signal};

#[cfg(unix)]
pub(super) struct ProcessTree {
    group: Option<i32>,
    #[cfg(target_os = "macos")]
    exited: Signal,
}

#[cfg(unix)]
impl ProcessTree {
    pub(super) fn spawn(
        command: &mut Command,
        _memory_bytes: Option<std::num::NonZeroU64>,
    ) -> io::Result<(Child, Self)> {
        #[cfg(target_os = "macos")]
        let exited = signal(SignalKind::child())?;
        command.process_group(0).kill_on_drop(true);
        let child = command.spawn()?;
        let group = child.id().map(|id| id as i32);
        Ok((
            child,
            Self {
                group,
                #[cfg(target_os = "macos")]
                exited,
            },
        ))
    }

    pub(super) fn request_stop(&mut self, child: &mut Child) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            if let Some(pid) = child.id() {
                self.observe_leader_exit(pid)?;
                let _ = child.start_kill();
            } else {
                self.group = None;
            }
            self.signal_group()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let result = self.signal_group();
            let _ = child.start_kill();
            if result.is_ok() {
                self.group = None;
            }
            result
        }
    }

    pub(super) async fn wait(&mut self, child: &mut Child) -> io::Result<ExitStatus> {
        #[cfg(target_os = "macos")]
        while self.group.is_some() {
            if let Some(status) = self.reap_after_group_sweep(child)? {
                return Ok(status);
            }
            self.exited
                .recv()
                .await
                .ok_or_else(|| io::Error::other("child exit notification closed"))?;
        }
        let status = child.wait().await?;
        self.finish_group()?;
        Ok(status)
    }

    pub(super) fn try_wait(&mut self, child: &mut Child) -> io::Result<Option<ExitStatus>> {
        #[cfg(target_os = "macos")]
        if self.group.is_some() {
            return self.reap_after_group_sweep(child);
        }
        let status = child.try_wait()?;
        if status.is_some() {
            self.finish_group()?;
        }
        Ok(status)
    }

    fn finish_group(&mut self) -> io::Result<()> {
        let result = self.signal_group();
        self.group = None;
        result
    }

    fn signal_group(&self) -> io::Result<()> {
        let Some(group) = self.group else {
            return Ok(());
        };
        // SAFETY: The positive child id names our dedicated process group; negative pid targets only that group. Rust has no safe group-signal API.
        let result = unsafe { libc::kill(-group, libc::SIGKILL) };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ESRCH) => Ok(()),
            #[cfg(target_os = "macos")]
            Some(libc::EPERM) if group_has_no_live_members(group) => Ok(()),
            _ => Err(error),
        }
    }

    #[cfg(any(target_os = "macos", all(target_os = "linux", test)))]
    fn reap_after_group_sweep(&mut self, child: &mut Child) -> io::Result<Option<ExitStatus>> {
        let Some(pid) = child.id() else {
            self.group = None;
            return child.try_wait();
        };
        if !self.observe_leader_exit(pid)? {
            return Ok(None);
        }
        // NOTE: Darwin snapshots group membership for each signal. Sweep again before releasing the exited leader's PID.
        self.signal_group()?;
        self.group = None;
        child.try_wait()
    }

    #[cfg(any(target_os = "macos", all(target_os = "linux", test)))]
    fn observe_leader_exit(&mut self, pid: u32) -> io::Result<bool> {
        exited_without_reaping(pid).inspect_err(|error| {
            if error.raw_os_error() == Some(libc::ECHILD) {
                self.group = None;
            }
        })
    }
}

#[cfg(target_os = "macos")]
fn group_has_no_live_members(group: libc::pid_t) -> bool {
    // SAFETY: A null buffer requests the capacity in PID slots. The leader's PID is still owned.
    let capacity = unsafe { libc::proc_listpgrppids(group, std::ptr::null_mut(), 0) };
    if capacity <= 0 {
        return false;
    }
    let Some(bytes) = capacity.checked_mul(size_of::<libc::pid_t>() as libc::c_int) else {
        return false;
    };
    let mut members = vec![0; capacity as usize];
    // SAFETY: The buffer has the advertised length. Clear thread-local errno to distinguish an empty list from an error.
    let count = unsafe {
        *libc::__error() = 0;
        libc::proc_listpgrppids(group, members.as_mut_ptr().cast(), bytes)
    };
    if count == 0 {
        return io::Error::last_os_error().raw_os_error() == Some(0);
    }
    // Darwin's group signal returns EPERM even if the only members are zombies.
    count > 0
        && count < capacity
        && members[..count as usize].iter().all(|&pid| {
            // SAFETY: getpgid queries the PID without signalling it and excludes zombies on Darwin.
            let member_group = unsafe { libc::getpgid(pid) };
            if member_group == -1 {
                io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            } else {
                member_group != group
            }
        })
}

#[cfg(any(target_os = "macos", all(target_os = "linux", test)))]
fn exited_without_reaping(pid: u32) -> io::Result<bool> {
    loop {
        // SAFETY: Zero is the no-event sentinel. waitid writes siginfo_t for this owned child without consuming its exit status.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: The flags keep this nonblocking and leave the observed child's PID owned until the subsequent reap.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == 0 {
            // SAFETY: The pid field is valid in the initialized event or remains the zero sentinel.
            return Ok(unsafe { info.si_pid() } != 0);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(unix)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            let Some(group) = self.group else {
                return;
            };
            if self.observe_leader_exit(group as u32).is_err() {
                return;
            }
        }
        let _ = self.finish_group();
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::num::NonZeroU64;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::process::ExitStatus;
    use std::ptr;

    use tokio::process::{Child, Command};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_JOB_MEMORY,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
        JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
        TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
    };

    pub(crate) struct ProcessTree {
        job: Option<OwnedHandle>,
    }

    impl ProcessTree {
        pub(crate) fn spawn(
            command: &mut Command,
            memory_bytes: Option<NonZeroU64>,
        ) -> io::Result<(Child, Self)> {
            let job = create_job(memory_bytes)?;
            command
                .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
                .kill_on_drop(true);
            let mut child = command.spawn()?;
            let result = assign_and_resume(&job, &child);
            if let Err(error) = result {
                let _ = child.start_kill();
                return Err(error);
            }
            Ok((child, Self { job: Some(job) }))
        }

        pub(crate) fn request_stop(&self, _child: &mut Child) -> io::Result<()> {
            self.terminate_owned()
        }

        pub(crate) async fn wait(&mut self, child: &mut Child) -> io::Result<ExitStatus> {
            let status = child.wait().await?;
            self.terminate_owned()?;
            Ok(status)
        }

        pub(crate) fn try_wait(&mut self, child: &mut Child) -> io::Result<Option<ExitStatus>> {
            let status = child.try_wait()?;
            if status.is_some() {
                self.terminate_owned()?;
            }
            Ok(status)
        }

        fn terminate_owned(&self) -> io::Result<()> {
            if let Some(job) = &self.job
                && !self.is_empty()?
            {
                // SAFETY: The retained handle identifies this command's Job Object.
                if unsafe { TerminateJobObject(job.as_raw_handle(), 1) } == 0 {
                    let error = io::Error::last_os_error();
                    if !self.is_empty()? {
                        return Err(error);
                    }
                }
            }
            Ok(())
        }

        fn is_empty(&self) -> io::Result<bool> {
            let Some(job) = &self.job else {
                return Ok(true);
            };
            // SAFETY: The buffer matches the requested accounting information
            // class and the retained Job Object handle is live.
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
            let read = unsafe {
                QueryInformationJobObject(
                    job.as_raw_handle(),
                    JobObjectBasicAccountingInformation,
                    (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    std::mem::size_of_val(&info) as u32,
                    ptr::null_mut(),
                )
            };
            if read == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(info.ActiveProcesses == 0)
        }

        pub(crate) async fn wait_for_empty(&self) -> io::Result<()> {
            while !self.is_empty()? {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Ok(())
        }
    }

    fn create_job(memory_bytes: Option<NonZeroU64>) -> io::Result<OwnedHandle> {
        // SAFETY: Null security/name pointers request an unnamed job with default security; the returned non-null handle is newly owned.
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateJobObjectW transferred a valid unique handle; OwnedHandle closes it on every exit path.
        let job = unsafe { OwnedHandle::from_raw_handle(raw) };
        // SAFETY: This Win32 C structure permits all-zero initialization before its documented limit flag is set.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Some(bytes) = memory_bytes {
            limits.JobMemoryLimit = usize::try_from(bytes.get()).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "command memory limit exceeds the platform size",
                )
            })?;
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
        }
        // SAFETY: The job handle and correctly sized limits remain valid throughout the call; Win32 exposes no safe wrapper.
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
        // SAFETY: Both handles are live and the child is suspended, so no descendant can escape before job assignment.
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
        // SAFETY: TH32CS_SNAPTHREAD ignores the process argument and returns a newly owned snapshot handle or INVALID_HANDLE_VALUE.
        let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: The successful snapshot call transferred this valid unique handle.
        let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
        // SAFETY: THREADENTRY32 is a zero-initializable Win32 C structure; dwSize is required before enumeration.
        let mut entry: THREADENTRY32 = unsafe { zeroed() };
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        // SAFETY: The snapshot and correctly sized writable entry remain live throughout enumeration.
        let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
        while found != 0 {
            if entry.th32OwnerProcessID == process_id {
                return resume_thread(entry.th32ThreadID);
            }
            // SAFETY: The same valid snapshot and entry are retained until enumeration finishes.
            found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
        }
        Err(io::Error::other(
            "suspended command primary thread not found",
        ))
    }

    fn resume_thread(thread_id: u32) -> io::Result<()> {
        // SAFETY: The suspended child's enumerated thread id is opened with only resume rights and no handle inheritance.
        let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: OpenThread transferred a valid unique handle that must be closed even when resuming fails.
        let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
        // SAFETY: This live handle has resume rights, and CREATE_SUSPENDED gave the primary thread one suspension count.
        if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(windows)]
pub(super) use windows::ProcessTree;

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use std::os::unix::process::ExitStatusExt;

    use tokio::process::Command;
    use tokio::time::{Duration, sleep, timeout};

    use super::ProcessTree;
    #[cfg(target_os = "macos")]
    use super::{exited_without_reaping, group_has_no_live_members};

    #[tokio::test]
    async fn pre_reap_sweep_stops_a_process_that_joined_after_the_first_signal() {
        let (mut leader, mut tree) =
            ProcessTree::spawn(Command::new("sleep").arg("30"), None).unwrap();
        let group = leader.id().unwrap() as i32;
        tree.signal_group().unwrap();

        let mut late = Command::new("sleep")
            .arg("30")
            .process_group(group)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        assert!(late.try_wait().unwrap().is_none());
        let leader_status = timeout(Duration::from_secs(2), async {
            loop {
                if let Some(status) = tree.reap_after_group_sweep(&mut leader).unwrap() {
                    break status;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let late_status = timeout(Duration::from_secs(2), late.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(Some(libc::SIGKILL), leader_status.signal());
        assert_eq!(Some(libc::SIGKILL), late_status.signal());
        assert_eq!(None, leader.id());
        assert_eq!(None, tree.group);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn zombie_only_group_can_be_swept_before_reaping_the_leader() {
        let (mut leader, mut tree) =
            ProcessTree::spawn(Command::new("sleep").arg("30"), None).unwrap();
        let group = leader.id().unwrap() as i32;
        let mut sibling = Command::new("sh")
            .args(["-c", "exit 7"])
            .process_group(group)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let sibling_pid = sibling.id().unwrap();
        timeout(Duration::from_secs(2), async {
            while !exited_without_reaping(sibling_pid).unwrap() {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(!group_has_no_live_members(group));

        tree.request_stop(&mut leader).unwrap();
        let status = timeout(Duration::from_secs(2), tree.wait(&mut leader))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(Some(libc::SIGKILL), status.signal());
        assert_eq!(Some(7), sibling.try_wait().unwrap().unwrap().code());
        assert_eq!(None, tree.group);
    }
}
