//! `KeyInspector` over `ssh-keygen -l -f /dev/stdin`: the key line goes to
//! stdin, the first output line is `<bits> SHA256:<fp> <comment> (<TYPE>)`.
//! A key is public data, so its output is captured (unlike the password
//! tools, whose output is discarded).

use crate::adapters::accounts_shadow::{CommandSpec, Runner, SystemRunner};
use crate::app::error::AppError;
use crate::ports::KeyInspector;

pub const SSH_KEYGEN: &str = "/usr/bin/ssh-keygen";

pub struct SshKeygen<R: Runner> {
    runner: R,
}

impl SshKeygen<SystemRunner> {
    pub fn new(runner: SystemRunner) -> SshKeygen<SystemRunner> {
        SshKeygen { runner }
    }
}

impl<R: Runner> SshKeygen<R> {
    pub fn with_runner(runner: R) -> SshKeygen<R> {
        SshKeygen { runner }
    }
}

impl<R: Runner> KeyInspector for SshKeygen<R> {
    fn fingerprint(&self, line: &str) -> Result<String, AppError> {
        if line.is_empty() || line.contains(['\n', '\r', '\0']) {
            return Err(AppError::invalid("not a single public key line"));
        }
        let mut spec = CommandSpec::new(SSH_KEYGEN, &["-l", "-f", "/dev/stdin"]);
        spec.stdin = format!("{line}\n").into_bytes();
        let (status, out) = self.runner.run_capture(&spec)?;
        if status != 0 {
            return Err(AppError::invalid("ssh-keygen does not accept this key"));
        }
        let text = String::from_utf8_lossy(&out);
        text.lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .filter(|f| f.starts_with("SHA256:") && f.len() > 7 && f.bytes().all(|b| b.is_ascii_graphic()))
            .map(str::to_string)
            .ok_or_else(|| AppError::failed("unexpected ssh-keygen output"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::error::ErrorKind;
    use std::sync::Mutex;

    type Call = (String, Vec<String>, Vec<u8>);

    struct Fake {
        calls: Mutex<Vec<Call>>,
        status: i32,
        out: &'static str,
    }

    impl Runner for &Fake {
        fn run(&self, _: &CommandSpec) -> Result<i32, AppError> {
            unreachable!("only run_capture is used")
        }

        fn run_capture(&self, spec: &CommandSpec) -> Result<(i32, Vec<u8>), AppError> {
            let args = spec.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
            self.calls.lock().unwrap().push((spec.program.to_string(), args, spec.stdin.clone()));
            Ok((self.status, self.out.as_bytes().to_vec()))
        }
    }

    #[test]
    fn argv_is_exact_and_the_key_goes_to_stdin() {
        let f = Fake { calls: Mutex::new(Vec::new()), status: 0, out: "256 SHA256:abc123XYZ ana@laptop (ED25519)\n" };
        let fp = SshKeygen::with_runner(&f).fingerprint("ssh-ed25519 AAAAC3 ana@laptop").unwrap();
        assert_eq!(fp, "SHA256:abc123XYZ");
        let calls = f.calls.lock().unwrap().clone();
        assert_eq!(calls[0].0, "/usr/bin/ssh-keygen");
        assert_eq!(calls[0].1, vec!["-l", "-f", "/dev/stdin"]);
        assert_eq!(calls[0].2, b"ssh-ed25519 AAAAC3 ana@laptop\n");
    }

    #[test]
    fn a_rejected_key_is_invalid_and_odd_output_is_a_failure() {
        let f = Fake { calls: Mutex::new(Vec::new()), status: 255, out: "" };
        let e = SshKeygen::with_runner(&f).fingerprint("ssh-ed25519 AAAA").unwrap_err();
        assert_eq!(e.kind, ErrorKind::Invalid);
        let g = Fake { calls: Mutex::new(Vec::new()), status: 0, out: "garbage\n" };
        let e = SshKeygen::with_runner(&g).fingerprint("ssh-ed25519 AAAA").unwrap_err();
        assert_eq!(e.kind, ErrorKind::Failed);
    }

    #[test]
    fn multi_line_input_never_reaches_the_tool() {
        let f = Fake { calls: Mutex::new(Vec::new()), status: 0, out: "" };
        for bad in ["", "a\nb", "a\rb", "a\0b"] {
            assert_eq!(SshKeygen::with_runner(&f).fingerprint(bad).unwrap_err().kind, ErrorKind::Invalid, "{bad:?}");
        }
        assert!(f.calls.lock().unwrap().is_empty());
    }
}
