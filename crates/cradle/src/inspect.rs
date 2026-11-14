//! `cradle providers` and `cradle config`: print what Cradle would load.

use std::path::PathBuf;
use std::{fs, io};

use cradle_config::{EntryBody, Mode, ProjectConfig, SourceKind};
use cradle_provider_spec::{Auth, Backend, ProviderSpec};

use crate::Error;

/// Compiled-in providers plus the runtime overlay. Also returns the overlay path if one
/// was applied.
pub fn load_providers() -> Result<(Vec<ProviderSpec>, Option<PathBuf>), Error> {
    let overlay_path = cradle_config::paths::providers_overlay();
    let overlay = match &overlay_path {
        Some(path) => match fs::read_to_string(path) {
            Ok(src) => Some((path, src)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(Error::Io {
                    path: path.clone(),
                    source,
                });
            }
        },
        None => None,
    };

    let providers = cradle_backends::providers(overlay.as_ref().map(|(_, src)| src.as_str()))
        .map_err(|errors| Error::Providers {
            path: overlay
                .as_ref()
                .map_or_else(|| PathBuf::from("providers.toml"), |(p, _)| p.to_path_buf()),
            errors,
        })?;

    Ok((providers, overlay.map(|(p, _)| p.clone())))
}

pub fn list_providers() -> Result<(), Error> {
    let (providers, overlay) = load_providers()?;
    match &overlay {
        Some(path) => println!("compiled-in providers + {}\n", path.display()),
        None => println!("compiled-in providers (no runtime overlay)\n"),
    }
    for provider in &providers {
        println!("{}", describe_provider(provider));
    }
    Ok(())
}

fn describe_provider(p: &ProviderSpec) -> String {
    let detail = match &p.backend {
        Backend::Http(http) => {
            let auth = match &http.auth {
                Auth::None => "no auth".to_owned(),
                Auth::ApiKey { env } => format!("key ${env}"),
            };
            let c = &http.capabilities;
            let caps: Vec<&str> = [
                (c.tools, "tools"),
                (c.streaming, "streaming"),
                (c.prompt_caching, "caching"),
                (c.reasoning, "reasoning"),
            ]
            .into_iter()
            .filter_map(|(on, name)| on.then_some(name))
            .collect();
            let chat_only = if c.tools { "" } else { "  [chat only]" };
            format!(
                "{}  {auth}  [{}]{chat_only}",
                http.base_url,
                caps.join(", ")
            )
        }
        Backend::Cli(cli) => match &cli.version {
            Some(v) => format!("binary `{}`  version {v}", cli.binary),
            None => format!("binary `{}`  version unchecked", cli.binary),
        },
    };
    let default = match &p.backend {
        Backend::Http(h) => h.default_model.as_deref(),
        Backend::Cli(c) => c.default_model.as_deref(),
    };
    let default = default
        .map(|m| format!("  default model {m}"))
        .unwrap_or_default();
    format!("{:<14} {:<18} {detail}{default}", p.id, p.kind().as_str())
}

pub fn print_config(config: &ProjectConfig) {
    let source = match config.source {
        SourceKind::Directory => ".agents/",
        SourceKind::File => "agents.toml",
    };
    println!("project  {}", config.root.display());
    println!("source   {source}\n");

    let d = &config.docs;
    println!("docs");
    for (name, doc) in [
        ("product", &d.product),
        ("rules", &d.rules),
        ("architecture", &d.architecture),
        ("features overview", &d.features_overview),
        ("build", &d.build),
        ("tests", &d.tests),
        ("deps", &d.deps),
    ] {
        println!("  {name:<18} {}", if doc.is_some() { "yes" } else { "-" });
    }

    for (title, entries) in [
        ("features", &config.features),
        ("roadmap", &config.roadmap),
        ("bugfixes", &config.bugfixes),
    ] {
        println!("\n{title} ({})", entries.len());
        for entry in entries {
            match &entry.body {
                EntryBody::File(_) => println!("  {}", entry.name),
                EntryBody::Inline(summary) => println!("  {}: {summary}", entry.name),
            }
        }
    }

    let m = &config.modes;
    println!("\nmodes (clash_handler = {:?})", m.clash_handler);
    let show = |label: &str, mode: &Option<Mode>| match mode {
        Some(mode) => println!("  {label:<8} {}", mode.path.display()),
        None => println!("  {label:<8} (global default)"),
    };
    show("plan", &m.plan);
    show("build", &m.build);
    for mode in &m.extra {
        println!(
            "  {:<8} {} ({:?})",
            mode.name,
            mode.path.display(),
            mode.access
        );
    }

    println!("\ncommands");
    for (name, command) in &config.commands {
        println!("  {name:<10} {command}");
    }
}
