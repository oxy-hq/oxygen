//! Nothing on the server writes `config.yml`.
//!
//! `POST /api/{ws}/databases` once wrote the working copy while classified
//! `FleetOk`, so adding a warehouse succeeded or failed depending on which pod
//! the load balancer picked. The IDE removal took the database and integration
//! writes away, and the onboarding reset took the last two: `config.yml` is
//! edited in the workspace's repository and reaches the server by compile.
//!
//! So `ConfigManager` carries no writer, and this test keeps it that way. A
//! new one needs a handler, and a handler that writes the working copy is a
//! route only a pod with a disk can serve, which is what is being retired.

use std::path::Path;

#[test]
fn config_manager_has_no_config_yml_writer() {
    let core = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/src/config");
    for file in ["manager.rs", "storage.rs"] {
        let src = std::fs::read_to_string(core.join(file))
            .unwrap_or_else(|e| panic!("read core's {file}: {e}"));
        assert!(
            !src.contains("write_config"),
            "{file} writes config.yml again. The server serves the compiled \
             revision; config.yml changes arrive by commit and compile, not by a \
             handler writing a working copy."
        );
    }
}
