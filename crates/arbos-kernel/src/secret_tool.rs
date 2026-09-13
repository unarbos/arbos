//! `secret`: use a key from the vault without ever seeing it.

use anyhow::{Result, bail};
use arbos_engine::secrets::{Config, resolve, store};
use arbos_engine::{Access, BoxFuture, Plan, PlanCx, RunCx, Tool, ToolOut, typed_schema};
use serde_json::Value;

pub struct Secret;

impl Tool for Secret {
    fn name(&self) -> &'static str {
        "secret"
    }
    fn schema(&self) -> Value {
        typed_schema(
            "secret",
            "Keys without seeing them: list names (.arbos/secrets.toml); use NAME sets $NAME in bash's env from now on (value redacted everywhere); revoke NAME. Never print a value.",
            &[
                ("action", "list (default), use, or revoke.", false, "string"),
                ("name", "Secret name.", false, "string"),
            ],
        )
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::none()))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        Box::pin(async move {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("list")
                .trim()
                .to_ascii_lowercase();
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let place = cx.place.path().to_path_buf();
            let readonly = cx.agent.readonly;
            let text = tokio::task::spawn_blocking(move || -> Result<String> {
                let config = Config::load(&place)?;
                match action.as_str() {
                    "list" => Ok(list(&config)),
                    "use" => {
                        if readonly {
                            bail!("secret use: a readonly agent may not take secrets into its environment");
                        }
                        let name = name.ok_or_else(|| anyhow::anyhow!("secret use needs name"))?;
                        validate(&name)?;
                        let source = match config.secrets.get(&name) {
                            Some(s) => s.clone(),
                            None if std::env::var_os(&name).is_some() => format!("env:{name}"),
                            None => bail!(
                                "no secret named {name} here. {}",
                                if config.secrets.is_empty() {
                                    "Add it to .arbos/secrets.toml under [secrets] as NAME = \"op://vault/item/field\" (or env:VAR, file:/path), or ask the user for its source.".to_string()
                                } else {
                                    format!(
                                        "Configured: {}. Add it to .arbos/secrets.toml under [secrets], or ask the user for its source.",
                                        config.secrets.keys().cloned().collect::<Vec<_>>().join(", ")
                                    )
                                }
                            ),
                        };
                        let value = resolve(&source)?;
                        let len = value.len();
                        store().grant(&name, value);
                        Ok(format!(
                            "{name} is now set in the environment of every bash command ({len} characters, from {}). Its value is redacted from tool results as [REDACTED:{name}]. Use it as ${name}; never print it.",
                            arbos_engine::secrets::kind_of(&source)
                        ))
                    }
                    "revoke" => {
                        let name = name.ok_or_else(|| anyhow::anyhow!("secret revoke needs name"))?;
                        Ok(if store().revoke(&name) {
                            format!("{name} is no longer provided to bash commands.")
                        } else {
                            format!("{name} was not in use.")
                        })
                    }
                    other => bail!("secret: action must be list, use, or revoke, not {other:?}"),
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!("secret task: {e}"))??;
            Ok(ToolOut::text(text))
        })
    }
}

fn list(config: &Config) -> String {
    let granted = store().granted_names();
    let mut lines = Vec::new();
    for (name, kind) in config.describe() {
        lines.push(format!(
            "{name} — {kind}{}",
            if granted.contains(&name) {
                " (in use)"
            } else {
                ""
            }
        ));
    }
    for name in &granted {
        if !config.secrets.contains_key(name) {
            lines.push(format!("{name} — kernel environment (in use)"));
        }
    }
    if lines.is_empty() {
        return "No secrets configured. Add .arbos/secrets.toml with a [secrets] table: NAME = \"op://vault/item/field\" | \"env:VAR\" | \"file:/path\". Names in the kernel's environment can be used directly with secret use NAME.".into();
    }
    lines.join("\n")
}

fn validate(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || name.starts_with(|c: char| c.is_ascii_digit())
    {
        bail!("secret names are environment-variable names: [A-Za-z_][A-Za-z0-9_]*");
    }
    Ok(())
}
