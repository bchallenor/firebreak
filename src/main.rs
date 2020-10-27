use std::prelude::v1::*;

use lazy_static::lazy_static;
use std::io;

use crate::os::OsNs;

mod conn;
mod host;
mod os;

lazy_static! {
    static ref INIT: () = {
        // Must be called before other threads are spawned
        let ns_result = OsNs::enter_new_user();
        #[cfg(not(test))]
        ns_result.expect("Failed to enter new user namespace");
        #[cfg(test)]
        ns_result.expect("Failed to enter new user namespace (note that tests must be run with `RUST_TEST_THREADS=1`)");

        env_logger::init();
    };
}

fn main() -> Result<(), io::Error> {
    *INIT;
    Ok(())
}
