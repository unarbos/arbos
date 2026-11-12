#![allow(dead_code, unused_imports)]
mod sync {
    pub mod user_error {
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub enum ErrorClass {
            Timeout,
            /// `device_offline`: no route at all (DNS, unreachable); else the relay does not answer.
            Offline {
                device_offline: bool,
            },
            /// A 4xx the relay explained (`message` is its sentence when it has one).
            Refused {
                message: Option<String>,
            },
            NotFound,
            Server,
            RateLimited,
            ContentRefused,
            Upload(UploadFault),
            AuthExpired,
            Unknown,
        }
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub enum UploadFault {
            Unreadable,
            TooLarge,
            Unsupported,
        }
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct UserError {
            /// What the user tried, as an infinitive phrase: "send your message", "save the secret".
            pub verb: String,
            pub class: ErrorClass,
            /// The request, when known: "POST /projects/01M5N1…ESK7X/avatar/generate".
            pub call: Option<String>,
            /// The untrimmed raw words (an `anyhow` chain, the relay's body, the OS error).
            pub raw: String,
            /// Unix ms when it happened.
            pub at_ms: u64,
        }
    }
    #[path = "../agent_avatar.rs"]
    pub mod agent_avatar;
    #[path = "../credentials.rs"]
    pub mod credentials;
    #[path = "../dto.rs"]
    pub mod dto;
    #[path = "../relay.rs"]
    pub mod relay;
}

mod model;
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    sync::relay::run().await
}

mod ui {
    pub mod avatars {
        pub fn color_index(id: &str) -> u8 {
            arbos_core::avatar::color_index(id)
        }
    }
}
