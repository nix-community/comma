//! Regression tests for `COMPLETE=<shell> comma` (see README's "Shell
//! completion" section). These invoke the compiled binary directly, since
//! the bug they guard against (`CompleteEnv` not intercepting the request,
//! so argument parsing runs and fails with "the following required
//! arguments were not provided") can only be observed at the `main`
//! entrypoint.

use std::process::Command;

#[test]
fn complete_env_registers_without_requiring_a_command() {
    for shell in ["bash", "elvish", "fish", "powershell", "zsh"] {
        let output = Command::new(env!("CARGO_BIN_EXE_comma"))
            .env("COMPLETE", shell)
            .output()
            .expect("failed to run comma binary");

        assert!(
            output.status.success(),
            "COMPLETE={shell} comma exited with {:?}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(
            output.stderr.is_empty(),
            "COMPLETE={shell} comma wrote to stderr: {}",
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(
            !output.stdout.is_empty(),
            "COMPLETE={shell} comma produced no registration script"
        );
    }
}
