use std::prelude::v1::*;

use lazy_static::lazy_static;
use std::io;

use crate::os::OsNs;

mod conn;
mod host;
mod os;

lazy_static! {
    static ref INIT: () = {
        // Check that we have permission to create a network namespace
        match OsNs::new_net() {
            Ok(_) => {},
            Err(err) if err.raw_os_error() == Some(libc::EPERM) => {
                // Try to acquire permission by entering a new user namespace
                // Note this must be done before other threads are spawned
                match OsNs::enter_new_user() {
                    Ok(_) => {},
                    Err(err) if err.raw_os_error() == Some(libc::EINVAL) => {
                        #[cfg(test)]
                        panic!("Cannot create new user namespace after program has become multithreaded. Either create the user namespace outside of the tests, with `unshare --map-root-user`, or run the tests with `RUST_TEST_THREADS=1`.");
                        #[cfg(not(test))]
                        unreachable!("Did not expect program to be multithreaded by this point");
                    }
                    Err(err) => Err(err).expect("Failed to create new user namespace"),
                }
            }
            Err(err) => Err(err).expect("Failed to create new network namespace"),
        }

        env_logger::init();
    };
}

fn main() -> Result<(), io::Error> {
    *INIT;
    Ok(())
}
