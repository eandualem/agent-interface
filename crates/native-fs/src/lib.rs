//! Small native filesystem bridge. Owns no application state or file paths.

/// Inspect the open descriptor's Darwin extended ACL, without following paths.
/// Requires explicit filesystem support and a successful security-property
/// query. Any ACL property, including an empty or deny-only ACL, returns true.
#[cfg(target_os = "macos")]
pub fn has_extended_acl(file: &std::fs::File) -> std::io::Result<bool> {
    use std::{ffi::c_void, io, mem::MaybeUninit, os::fd::AsRawFd};
    unsafe extern "C" {
        fn filesec_init() -> *mut c_void;
        fn filesec_free(security: *mut c_void);
        fn filesec_query_property(
            security: *mut c_void,
            property: libc::c_int,
            present: *mut libc::c_int,
        ) -> libc::c_int;
        #[cfg_attr(
            any(target_arch = "x86", target_arch = "x86_64"),
            link_name = "fstatx_np$INODE64"
        )]
        fn fstatx_np(fd: libc::c_int, stat: *mut libc::stat, security: *mut c_void) -> libc::c_int;
    }
    // SAFETY: file stays open throughout; stat/present are correctly aligned
    // writable out-parameters. The security allocation is freed exactly once
    // and no pointers or uninitialized stat values escape this function.
    unsafe {
        if libc::fpathconf(file.as_raw_fd(), libc::_PC_EXTENDED_SECURITY_NP) != 1 {
            return Err(io::Error::from(io::ErrorKind::Unsupported));
        }
        let security = filesec_init();
        if security.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut stat = MaybeUninit::<libc::stat>::uninit();
        let mut present = 0;
        let result = if fstatx_np(file.as_raw_fd(), stat.as_mut_ptr(), security) != 0
            || filesec_query_property(security, 5, &mut present) != 0
        // FILESEC_ACL
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(present != 0)
        };
        filesec_free(security);
        result
    }
}

/// Start `command` with no signals blocked. Grand Central Dispatch workers block
/// every signal, and a child inherits that mask across exec, so a service started
/// from one would otherwise never see SIGTERM and could not be stopped.
#[cfg(target_os = "macos")]
pub fn clear_signal_mask(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the closure runs between fork and exec and only calls
    // async-signal-safe functions on its own stack value.
    unsafe {
        command.pre_exec(|| {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            match libc::pthread_sigmask(libc::SIG_SETMASK, &set, std::ptr::null_mut()) {
                0 => Ok(()),
                code => Err(std::io::Error::from_raw_os_error(code)),
            }
        });
    }
}

#[cfg(all(test, target_os = "macos"))]
mod signal_mask_tests {
    use std::{
        os::unix::process::ExitStatusExt,
        process::Command,
        time::{Duration, Instant},
    };

    #[test]
    fn a_child_of_a_thread_blocking_signals_still_receives_sigterm() {
        let spawned = std::thread::spawn(|| {
            // SAFETY: changes only this test thread's own mask.
            unsafe {
                let mut all: libc::sigset_t = std::mem::zeroed();
                libc::sigfillset(&mut all);
                libc::pthread_sigmask(libc::SIG_SETMASK, &all, std::ptr::null_mut());
            }
            let mut command = Command::new("/bin/sleep");
            command.arg("30");
            super::clear_signal_mask(&mut command);
            command.spawn().unwrap()
        });
        let mut child = spawned.join().unwrap();
        // SAFETY: signals only the child this test just started.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert_eq!(status.signal(), Some(libc::SIGTERM));
                return;
            }
            if Instant::now() > deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("the child never received SIGTERM");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
