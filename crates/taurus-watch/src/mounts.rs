//! Filesystems whose changes the operating system cannot report.
//!
//! A network share changed from another machine raises no event on this one,
//! and FUSE filesystems, WSL's view of Windows drives, and 9P mounts in
//! containers report some changes and not others. A watcher on one of those
//! answers with confidence and is wrong. So it is not started there, and the
//! consumer walks — the same list Zed's watcher polls on, and the same refusal
//! git's built-in fsmonitor daemon makes for network mounts by default.

use std::path::Path;

/// The filesystem `path` lives on, when it is one of those. `None` for a local
/// disk, and also when the question cannot be asked, since refusing to watch
/// a local disk would only cost speed.
#[cfg(target_os = "linux")]
pub(crate) fn unreliable(path: &Path) -> Option<&'static str> {
    let stat = rustix::fs::statfs(path).ok()?;
    // The magic numbers from `statfs(2)` and the kernel's `magic.h`. Widened to
    // one type because the field's own type differs by architecture.
    let kind = i128::from(stat.f_type);
    Some(match kind {
        0x6969 => "NFS",
        0x517b => "SMB",
        0xff53_4d42 => "CIFS",
        0xfe53_4d42 => "SMB2",
        0x6573_5546 => "FUSE",
        0x0102_1997 => "9P",
        0x7375_7245 => "Coda",
        0x5346_414f => "AFS",
        0x00c3_6400 => "Ceph",
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
pub(crate) fn unreliable(path: &Path) -> Option<&'static str> {
    let stat = rustix::fs::statfs(path).ok()?;
    let name: Vec<u8> = stat
        .f_fstypename
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    Some(match name.as_slice() {
        b"nfs" => "NFS",
        b"smbfs" => "SMB",
        b"afpfs" => "AFP",
        b"webdav" => "WebDAV",
        b"cifs" => "CIFS",
        b"macfuse" | b"osxfuse" | b"fusefs" => "FUSE",
        _ => return None,
    })
}

/// Windows is not asked: the sweep does not trust a watcher there at all yet.
/// See the crate header.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn unreliable(_path: &Path) -> Option<&'static str> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_temporary_directory_is_on_a_local_disk() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(super::unreliable(dir.path()), None);
    }
}
