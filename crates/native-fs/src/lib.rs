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
