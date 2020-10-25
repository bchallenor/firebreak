use std::prelude::v1::*;

use log::*;
use std::ffi::OsStr;
use std::fmt::Debug;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;

#[derive(Debug)]
pub struct OsNs {
    fd: File,
    /// Absolute path to the namespace. Valid only for the lifetime of this struct.
    fd_path: Box<Path>,
}

impl OsNs {
    pub fn enter_new_user() -> Result<(), io::Error> {
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        info!("uid {} gid {}", uid, gid);

        unsafe {
            let res = libc::unshare(libc::CLONE_NEWUSER);
            if res == -1 {
                return Err(io::Error::last_os_error());
            }
            assert_eq!(res, 0);
        }

        fs::write("/proc/self/setgroups", "deny")?;
        fs::write("/proc/self/uid_map", format!("0 {} 1", uid))?;
        fs::write("/proc/self/gid_map", format!("0 {} 1", gid))?;

        {
            let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
            info!("uid {} gid {}", uid, gid);
        }

        Ok(())
    }

    pub fn new_net() -> Result<OsNs, io::Error> {
        std::thread::spawn(|| {
            unsafe {
                let res = libc::unshare(libc::CLONE_NEWNET);
                if res == -1 {
                    return Err(io::Error::last_os_error());
                }
                assert_eq!(res, 0);
            }

            let current_path = "/proc/thread-self/ns/net";
            let fd = OpenOptions::new().read(true).open(current_path)?;
            let fd_path = PathBuf::from(format!(
                "/proc/{}/fd/{}",
                std::process::id(),
                fd.as_raw_fd()
            ))
            .into_boxed_path();
            Ok(OsNs { fd, fd_path })
        })
        .join()
        .unwrap()
    }

    pub fn path(&self) -> &Path {
        &self.fd_path
    }

    pub fn scoped<'a, F, T>(&self, f: F) -> Result<T, io::Error>
    where
        F: FnOnce() -> Result<T, io::Error>,
        F: Send + 'a,
        T: Send + 'a,
    {
        crossbeam_utils::thread::scope(|s| self.spawn_scoped(s, f).join().unwrap()).unwrap()
    }

    fn spawn_scoped<'scope, 'env, F, T>(
        &'env self,
        s: &'scope crossbeam_utils::thread::Scope<'env>,
        f: F,
    ) -> crossbeam_utils::thread::ScopedJoinHandle<'scope, Result<T, io::Error>>
    where
        F: FnOnce() -> Result<T, io::Error>,
        F: Send + 'env,
        T: Send + 'env,
    {
        s.spawn(move |_| {
            unsafe {
                let res = libc::setns(self.fd.as_raw_fd(), 0);
                if res == -1 {
                    return Err(io::Error::last_os_error());
                }
                assert_eq!(res, 0);
            }
            debug!("Spawned thread in namespace: {:?}", self.fd);
            f()
        })
    }

    pub fn scoped_process<S>(&self, program: &str, args: &[S]) -> Result<String, io::Error>
    where
        S: AsRef<OsStr> + Debug + Sync,
    {
        self.scoped_process_with_input(program, args, <&[u8]>::from(&[]))
    }

    pub fn scoped_process_with_input<S, R>(
        &self,
        program: &str,
        args: &[S],
        mut input: R,
    ) -> Result<String, io::Error>
    where
        S: AsRef<OsStr> + Debug + Sync,
        R: io::Read + Send,
    {
        self.scoped(|| {
            let mut p = Command::new(program)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap_or_else(|_| panic!("Failed to spawn: {} {:?}", program, args));

            let mut stdin = p.stdin.take().expect("stdin was not piped");
            io::copy(&mut input, &mut stdin)?;
            drop(stdin);

            let output = p.wait_with_output()?;
            let stdout = String::from_utf8(output.stdout)
                .unwrap_or_else(|_| panic!("{} {:?} stdout was not UTF-8", program, args));
            let stderr = String::from_utf8(output.stderr)
                .unwrap_or_else(|_| panic!("{} {:?} stderr was not UTF-8", program, args));
            assert!(
                output.status.success(),
                "{} {:?} returned {}:\n{}",
                program,
                args,
                output.status,
                stderr
            );
            Ok(stdout)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::INIT;

    #[test]
    fn add_veth_link() -> Result<(), io::Error> {
        *INIT;
        let ns = OsNs::new_net()?;
        ns.scoped_process(
            "ip",
            &[
                "link", "add", "veth0", "type", "veth", "peer", "name", "veth1",
            ],
        )?;
        let links = ns.scoped_process("ip", &["link"])?;
        debug!("{}", links);
        assert!(links.contains("veth0@veth1"));
        assert!(links.contains("veth1@veth0"));
        Ok(())
    }
}
