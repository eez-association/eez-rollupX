/// Finish CLI parsing at a process entrypoint, not inside a fallible parser.
/// Clap represents help/version as errors too: propagating them through `main`
/// turns successful display requests into exit 1 and writes them to stderr.
pub(crate) fn finish<T>(result: eyre::Result<T>) -> eyre::Result<T> {
    if let Err(report) = &result
        && let Some(error) = report.downcast_ref::<clap::Error>()
    {
        error.exit();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn cli_result_child() {
        let Ok(argument) = std::env::var("EEZ_CLI_RESULT_TEST_ARGUMENT") else {
            return;
        };
        let parsed = clap::Command::new("cli-fixture")
            .version("1.0.0")
            .try_get_matches_from(["cli-fixture", argument.as_str()]);
        finish(parsed.map_err(Into::into)).unwrap();
        panic!("display/error handling must exit before application startup");
    }

    #[test]
    fn clap_display_and_invalid_arguments_keep_their_exit_codes_and_streams() {
        for argument in ["--help", "-h", "--version", "-V", "--invalid-cli-fixture"] {
            let child = Command::new(std::env::current_exe().unwrap())
                .args(["cli_result_child", "--nocapture", "--test-threads=1"])
                .env("EEZ_CLI_RESULT_TEST_ARGUMENT", argument)
                .output()
                .unwrap();
            let stdout = String::from_utf8(child.stdout).unwrap();
            let stderr = String::from_utf8(child.stderr).unwrap();
            if argument == "--invalid-cli-fixture" {
                assert_eq!(child.status.code(), Some(2));
                assert!(stderr.contains("unexpected argument '--invalid-cli-fixture'"));
                assert!(!stdout.contains("Usage:"));
            } else {
                assert_eq!(child.status.code(), Some(0), "{argument}: {stderr}");
                assert!(stderr.is_empty(), "{argument}: {stderr}");
                assert!(stdout.contains(if argument.contains('h') {
                    "Usage:"
                } else {
                    "cli-fixture 1.0.0"
                }));
            }
        }
    }

    #[test]
    fn application_results_remain_fallible() {
        assert_eq!(finish(Ok(42)).unwrap(), 42);
        let error = finish::<()>(Err(eyre::eyre!("invalid chain document"))).unwrap_err();
        assert_eq!(error.to_string(), "invalid chain document");
    }
}
