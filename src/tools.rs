//! The tool surface, as `ecp` itself describes it. `ecp admin mcp tools
//! --format json` prints every MCP tool with its JSON schema and the argv
//! rules (`positional_args`, `flag_args`, `prefix_args`, `subcmd_arg`); this
//! module loads that list, keeps the read-only subset, and rebuilds argv the
//! way the MCP server does (`ecp-mcp`'s `argv::json_to_argv` and
//! `spawn::peel_subcmd`).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

/// Read-only subcommands. Left out on purpose: `rename` edits files;
/// `uninstall` and `admin` mutate the host; `peers`, `group`, `usage`,
/// `review` and `diff` read session, group, telemetry or working-tree state
/// a fresh checkout never carries.
pub const ALLOWED: &[&str] = &[
    "find",
    "inspect",
    "impact",
    "routes",
    "contracts",
    "path",
    "cypher",
    "summary",
    "processes",
    "tool-map",
    "shape-check",
    "heuristics",
    "pattern",
    "schema",
];

/// Flags the server owns. `repo` is set from the selected checkout, `graph`
/// would let a caller point `ecp` at any file in the container (it is a
/// clap global, so every subcommand accepts it), `batch` reads stdin the
/// demo never provides.
pub const RESERVED_ARGS: &[&str] = &["repo", "graph", "batch"];

/// One entry of `ecp admin mcp tools --format json`.
#[derive(Debug, Clone, Deserialize)]
pub struct Tool {
    pub name: String,
    pub subcommand: String,
    pub description: String,
    pub schema: Value,
    /// Arg ids emitted as a bare `--flag` when true and dropped when false.
    #[serde(default)]
    pub flag_args: BTreeSet<String>,
    /// Arg ids passed as bare values, in declared order, ahead of the flags.
    #[serde(default)]
    pub positional_args: Vec<String>,
    /// Fixed tokens ahead of the JSON-derived args (sub-subcommand routers).
    #[serde(default)]
    pub prefix_args: Vec<String>,
    /// JSON key lifted out and placed first as the sub-subcommand name.
    #[serde(default)]
    pub subcmd_arg: Option<String>,
}

#[derive(Debug)]
pub struct DemoTool {
    pub inner: Tool,
    /// Whether `--repo` exists on this subcommand; the runner injects it only then.
    pub takes_repo: bool,
    /// `inner.schema` with the reserved args removed from `properties` and `required`.
    pub public_schema: Value,
}

#[derive(Serialize)]
pub struct ToolListing<'a> {
    pub name: &'a str,
    pub subcommand: &'a str,
    pub description: &'a str,
    pub schema: &'a Value,
    pub positional_args: &'a [String],
}

impl DemoTool {
    fn new(inner: Tool) -> Self {
        let mut public_schema = inner.schema.clone();
        let takes_repo = public_schema
            .get("properties")
            .and_then(|p| p.get("repo"))
            .is_some();
        if let Some(props) = public_schema
            .get_mut("properties")
            .and_then(Value::as_object_mut)
        {
            for key in RESERVED_ARGS {
                props.remove(*key);
            }
        }
        if let Some(required) = public_schema
            .get_mut("required")
            .and_then(Value::as_array_mut)
        {
            required.retain(|v| !RESERVED_ARGS.iter().any(|r| v.as_str() == Some(r)));
        }
        Self {
            inner,
            takes_repo,
            public_schema,
        }
    }

    pub fn listing(&self) -> ToolListing<'_> {
        ToolListing {
            name: &self.inner.name,
            subcommand: &self.inner.subcommand,
            description: &self.inner.description,
            schema: &self.public_schema,
            positional_args: &self.inner.positional_args,
        }
    }
}

/// Ask the `ecp` binary for its tool list; the allowlisted subset, in
/// `ecp --help` order.
pub fn load_tools(bin: &Path) -> anyhow::Result<Vec<DemoTool>> {
    let out = std::process::Command::new(bin)
        .env_clear()
        .envs(crate::spawn::ecp_env())
        .args(["admin", "mcp", "tools", "--format", "json"])
        .output()
        .map_err(|e| anyhow::anyhow!("running {} admin mcp tools: {e}", bin.display()))?;
    anyhow::ensure!(
        out.status.success(),
        "{} admin mcp tools --format json failed: {}",
        bin.display(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    demo_tools_from_json(&String::from_utf8_lossy(&out.stdout))
}

pub fn demo_tools_from_json(json: &str) -> anyhow::Result<Vec<DemoTool>> {
    let tools: Vec<Tool> = serde_json::from_str(json).map_err(|e| {
        anyhow::anyhow!(
            "ecp admin mcp tools --format json: {e}; ecp ≥ 0.13.1 prints the full tool list"
        )
    })?;
    let demo: Vec<DemoTool> = tools
        .into_iter()
        .filter(|t| ALLOWED.contains(&t.subcommand.as_str()))
        .map(DemoTool::new)
        .collect();
    anyhow::ensure!(!demo.is_empty(), "no allowlisted tool in the ecp tool list");
    Ok(demo)
}

/// Every argv token after `<subcommand>`: the peeled sub-subcommand (if the
/// tool routes one), the tool's fixed prefix, then the JSON-derived args.
pub fn build_argv(tool: &Tool, args: &Value) -> Result<Vec<String>, String> {
    let (peeled_subcmd, json_args) = peel_subcmd(tool, args)?;
    let json_argv = json_to_argv(&json_args, &tool.flag_args, &tool.positional_args)?;
    Ok(peeled_subcmd
        .into_iter()
        .chain(tool.prefix_args.iter().cloned())
        .chain(json_argv)
        .collect())
}

/// `Rust ident → --kebab-flag`, as clap derive names flags: `includeTests`
/// and `include_tests` both become `--include-tests`.
fn to_kebab(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else if c == '_' {
            out.push('-');
        } else {
            out.push(c);
        }
    }
    out
}

/// Positionals first, in declared order; then one `--flag [value]` per
/// remaining key. A boolean in `flag_args` is a bare flag when true and
/// absent when false; any other boolean is passed as `--flag true|false`.
fn json_to_argv(
    args: &Value,
    flag_args: &BTreeSet<String>,
    positional_args: &[String],
) -> Result<Vec<String>, String> {
    let Value::Object(map) = args else {
        return Err(format!(
            "expected a JSON object for args, got {}",
            type_name(args)
        ));
    };
    let mut out = Vec::with_capacity(map.len() * 2);
    for pos_id in positional_args {
        if let Some(s) = map.get(pos_id).and_then(value_as_string) {
            out.push(s);
        }
    }
    for (k, v) in map {
        if positional_args.iter().any(|p| p == k) {
            continue;
        }
        let flag = format!("--{}", to_kebab(k));
        match v {
            Value::Null => continue,
            Value::Bool(b) => {
                if flag_args.contains(k) {
                    if *b {
                        out.push(flag);
                    }
                } else {
                    out.push(flag);
                    out.push(b.to_string());
                }
            }
            Value::String(s) => {
                out.push(flag);
                out.push(s.clone());
            }
            Value::Number(n) => {
                out.push(flag);
                out.push(n.to_string());
            }
            Value::Array(_) | Value::Object(_) => {
                return Err(format!("nested array/object args not supported (key={k})"));
            }
        }
    }
    Ok(out)
}

/// For a router tool, lift `subcmd_arg` out of the args, check it against
/// the schema's enum, and return it with the remaining args.
fn peel_subcmd(tool: &Tool, args: &Value) -> Result<(Option<String>, Value), String> {
    let Some(key) = tool.subcmd_arg.as_deref() else {
        return Ok((None, args.clone()));
    };
    let map = args
        .as_object()
        .ok_or_else(|| format!("expected a JSON object for args of {}", tool.name))?;
    let val = map
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing required `{key}` discriminator"))?
        .to_string();
    if let Some(allowed) = tool.schema["properties"][key]["enum"].as_array() {
        if !allowed.iter().any(|s| s.as_str() == Some(&val)) {
            return Err(format!("`{key}` must be one of {allowed:?}, got {val:?}"));
        }
    }
    let mut filtered = map.clone();
    filtered.remove(key);
    Ok((Some(val), Value::Object(filtered)))
}

fn value_as_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// The first reserved flag among argv tokens, matched the way clap parses
/// them: `--graph`, `--graph=…`. Checked on the translated argv rather than
/// on JSON keys, so key spelling (`Graph` → `--graph`) and a positional
/// value that starts with `--` are both caught.
pub fn reserved_token(argv: &[String]) -> Option<&'static str> {
    argv.iter().find_map(|token| {
        let name = token.strip_prefix("--")?.split('=').next()?;
        RESERVED_ARGS.iter().copied().find(|r| *r == name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Captured from `ecp admin mcp tools --format json`; the api tests feed
    /// the same file to their stub `ecp`.
    pub const FIXTURE: &str = include_str!("../tests/fixtures/tools.json");

    fn tools() -> Vec<DemoTool> {
        demo_tools_from_json(FIXTURE).unwrap()
    }

    fn tool(sub: &str) -> DemoTool {
        tools()
            .into_iter()
            .find(|t| t.inner.subcommand == sub)
            .unwrap_or_else(|| panic!("{sub} is allowlisted"))
    }

    #[test]
    fn demo_tools_exclude_every_mutating_or_stateful_subcommand() {
        let tools = tools();
        let names: Vec<&str> = tools.iter().map(|t| t.inner.subcommand.as_str()).collect();
        for banned in [
            "rename",
            "uninstall",
            "peers",
            "group",
            "usage",
            "review",
            "diff",
        ] {
            assert!(!names.contains(&banned), "{banned} must not be exposed");
        }
        assert_eq!(
            names.len(),
            ALLOWED.len(),
            "every allowlisted subcommand resolves: {names:?}"
        );
    }

    #[test]
    fn public_schema_drops_reserved_args_but_keeps_the_rest() {
        let find = tool("find");
        assert!(find.takes_repo);
        let props = find.public_schema["properties"].as_object().unwrap();
        for reserved in RESERVED_ARGS {
            assert!(
                !props.contains_key(*reserved),
                "{reserved} leaked into the public schema"
            );
        }
        assert!(
            props.contains_key("pattern"),
            "positional `pattern` survives"
        );
        assert!(props.contains_key("mode"), "`--mode` survives");
    }

    #[test]
    fn build_argv_puts_positionals_first_and_bare_flags_without_values() {
        let find = tool("find");
        let argv = build_argv(
            &find.inner,
            &json!({"mode": "fuzzy", "pattern": "x", "all": true}),
        )
        .unwrap();
        assert_eq!(argv[0], "x", "positional leads: {argv:?}");
        assert!(
            argv.contains(&"--all".to_string()) && !argv.contains(&"true".to_string()),
            "{argv:?}"
        );
        let flag_pos = argv.iter().position(|t| t == "--mode").unwrap();
        assert_eq!(argv[flag_pos + 1], "fuzzy");
        let argv = build_argv(&find.inner, &json!({"pattern": "x", "all": false})).unwrap();
        assert_eq!(argv, ["x"], "a false bare flag is dropped");
    }

    #[test]
    fn build_argv_kebab_cases_keys_and_rejects_nested_values() {
        let inspect = tool("inspect");
        let argv = build_argv(&inspect.inner, &json!({"name": "f", "includeTests": true})).unwrap();
        assert!(argv.iter().any(|t| t == "--include-tests"), "{argv:?}");
        let err = build_argv(&inspect.inner, &json!({"name": "f", "kind": ["a"]})).unwrap_err();
        assert!(err.contains("nested"), "{err}");
    }

    #[test]
    fn build_argv_peels_the_router_discriminator_and_validates_it() {
        let schema = tool("schema");
        assert_eq!(schema.inner.subcmd_arg.as_deref(), Some("subcmd"));
        let argv = build_argv(&schema.inner, &json!({"subcmd": "node-kinds"})).unwrap();
        assert_eq!(argv[0], "node-kinds");
        assert!(build_argv(&schema.inner, &json!({}))
            .unwrap_err()
            .contains("subcmd"));
        assert!(build_argv(&schema.inner, &json!({"subcmd": "nope"}))
            .unwrap_err()
            .contains("must be one of"));
    }

    #[test]
    fn reserved_token_catches_key_spelling_and_positional_smuggling() {
        let find = tool("find");
        let argv = |args: Value| build_argv(&find.inner, &args).unwrap();
        assert_eq!(
            reserved_token(&argv(json!({"pattern": "x", "graph": "/etc/passwd"}))),
            Some("graph")
        );
        assert_eq!(
            reserved_token(&argv(json!({"pattern": "x", "Graph": "/etc/passwd"}))),
            Some("graph"),
            "`Graph` kebab-cases to --graph"
        );
        assert_eq!(
            reserved_token(&argv(json!({"pattern": "--graph=/etc/shadow"}))),
            Some("graph"),
            "a positional value is passed verbatim and clap reads it as the flag"
        );
        assert_eq!(
            reserved_token(&argv(json!({"pattern": "x", "repo": "/"}))),
            Some("repo")
        );
        assert_eq!(
            reserved_token(&argv(json!({"pattern": "x", "mode": "fuzzy", "all": true}))),
            None
        );
        assert_eq!(
            reserved_token(&argv(json!({"pattern": "graph"}))),
            None,
            "a bare word is not a flag"
        );
    }

    #[test]
    fn demo_tools_from_json_rejects_the_old_name_only_shape() {
        let err = demo_tools_from_json(r#"[{"name":"ecp_find","description":"x"}]"#).unwrap_err();
        assert!(err.to_string().contains("0.13.1"), "{err}");
    }
}
