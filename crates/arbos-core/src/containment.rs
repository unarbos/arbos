//! Containment: the ways a command reaches past the machine it was given.
//!
//! An agent led astray by something it read can try to fetch the cloud's
//! instance credentials from the metadata service, talk to the container
//! runtime's socket, or read the user's cloud, kube, and ssh credentials.
//! None of that is part of ordinary work, so a command that names one of
//! these asks the user first, in every mode (Claude Code's auto-mode
//! "Containment Escape" rule), unless `.arbos/sandbox.toml` marks the
//! environment as expecting it (`allow_metadata = true`, for a task that
//! really runs on a cloud instance and needs its role).

/// Cloud metadata services: instance credentials, one HTTP call away.
pub const METADATA_HOSTS: &[&str] = &[
    "169.254.169.254", // AWS, Azure, GCP (legacy), OpenStack, DigitalOcean
    "fd00:ec2::254",   // AWS IPv6
    "169.254.170.2",   // ECS task credentials
    "100.100.100.200", // Alibaba Cloud
    "metadata.google.internal",
    "metadata.azure.com",
    "metadata.oraclecloud.com",
];

/// The container runtime and privilege escapes.
const RUNTIME_ESCAPES: &[&str] = &[
    "/var/run/docker.sock",
    "/run/docker.sock",
    "/run/containerd/containerd.sock",
    "/var/run/crio/crio.sock",
    "nsenter ",
    "--privileged",
];

/// Credential files on the user's machine. Read as file names, so a
/// `cat`, `cp`, `base64`, `curl -F @…` all count.
const CREDENTIAL_PATHS: &[&str] = &[
    ".aws/credentials",
    ".config/gcloud/",
    ".azure/",
    ".kube/config",
    ".ssh/id_",
    ".netrc",
    ".docker/config.json",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
];

/// What a command reaches for, or None. The label names the class so the
/// approval line can say it plainly.
pub fn risk_of(command: &str) -> Option<&'static str> {
    let c = command.to_ascii_lowercase();
    if METADATA_HOSTS.iter().any(|h| c.contains(h)) {
        return Some("the cloud metadata service (instance credentials)");
    }
    if RUNTIME_ESCAPES.iter().any(|p| c.contains(p)) {
        return Some("the container runtime or a privileged namespace");
    }
    if CREDENTIAL_PATHS.iter().any(|p| c.contains(p)) {
        return Some("credential files on this machine");
    }
    None
}

/// Whether a URL points at a metadata service: for `fetch`, which has no
/// reason to ever read one.
pub fn url_is_metadata(url: &str) -> bool {
    let u = url.to_ascii_lowercase();
    METADATA_HOSTS.iter().any(|h| u.contains(h))
}

/// The line the user sees when a command reaches for one of these.
pub fn question(tool: &str, risk: &str) -> String {
    format!("{tool} reaches for {risk}, which ordinary work here does not need. Allow it?")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_runtime_and_credential_reaches_are_named_and_ordinary_commands_are_not() {
        assert_eq!(
            risk_of("curl -s http://169.254.169.254/latest/meta-data/iam/security-credentials/"),
            Some("the cloud metadata service (instance credentials)")
        );
        assert_eq!(
            risk_of(
                "curl -H 'Metadata-Flavor: Google' http://metadata.google.internal/computeMetadata/v1/"
            ),
            Some("the cloud metadata service (instance credentials)")
        );
        assert_eq!(
            risk_of("docker -H unix:///var/run/docker.sock run --privileged alpine"),
            Some("the container runtime or a privileged namespace")
        );
        assert_eq!(
            risk_of("cat ~/.aws/credentials | base64"),
            Some("credential files on this machine")
        );
        assert_eq!(
            risk_of("cat ~/.ssh/id_ed25519"),
            Some("credential files on this machine")
        );
        for ok in [
            "cargo test",
            "curl -s https://api.github.com/repos/o/r",
            "ssh -T git@github.com",
            "docker build -t x .",
            "ls ~/.ssh",
            "grep -r metadata src/",
        ] {
            assert_eq!(risk_of(ok), None, "{ok}");
        }
        assert!(url_is_metadata("http://169.254.169.254/latest/api/token"));
        assert!(!url_is_metadata("https://docs.aws.amazon.com/"));
    }
}
