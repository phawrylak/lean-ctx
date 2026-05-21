use std::io::Read as _;
use std::path::PathBuf;

pub(crate) fn cmd_pack(args: &[String]) {
    let project_root = super::common::detect_project_root(args);

    let subcommand = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map_or("pr", String::as_str);

    match subcommand {
        "pr" => cmd_pack_pr(args, &project_root),
        "create" => cmd_pack_create(args, &project_root),
        "install" => cmd_pack_install(args, &project_root),
        "list" | "ls" => cmd_pack_list(),
        "info" => cmd_pack_info(args),
        "remove" | "rm" => cmd_pack_remove(args),
        "export" => cmd_pack_export(args),
        "import" => cmd_pack_import(args, &project_root),
        "auto-load" => cmd_pack_auto_load(args),
        "send" => cmd_pack_send(args, &project_root),
        "receive" => cmd_pack_receive(args, &project_root),
        "help" | "--help" | "-h" => print_usage(),
        other => {
            eprintln!("Unknown pack subcommand: {other}");
            print_usage();
        }
    }
}

fn cmd_pack_pr(args: &[String], project_root: &str) {
    let mut base: Option<String> = None;
    let mut format: Option<String> = None;
    let mut depth: Option<usize> = None;
    let mut diff_from_stdin = false;

    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        if a == "pr" {
            continue;
        }
        if let Some(v) = a.strip_prefix("--base=") {
            base = Some(v.to_string());
            continue;
        }
        if a == "--base" {
            if let Some(v) = it.peek() {
                if !v.starts_with("--") {
                    base = Some((*v).clone());
                    it.next();
                }
            }
            continue;
        }
        if let Some(v) = a.strip_prefix("--format=") {
            format = Some(v.to_string());
            continue;
        }
        if a == "--format" {
            if let Some(v) = it.peek() {
                if !v.starts_with("--") {
                    format = Some((*v).clone());
                    it.next();
                }
            }
            continue;
        }
        if a == "--json" {
            format = Some("json".to_string());
            continue;
        }
        if let Some(v) = a.strip_prefix("--depth=") {
            depth = v.parse::<usize>().ok();
            continue;
        }
        if a == "--depth" {
            if let Some(v) = it.peek() {
                if !v.starts_with("--") {
                    depth = (*v).parse::<usize>().ok();
                    it.next();
                }
            }
            continue;
        }
        if a == "--diff-from-stdin" {
            diff_from_stdin = true;
        }
    }

    let diff = if diff_from_stdin {
        let mut buf = String::new();
        let _ = std::io::stdin().read_to_string(&mut buf);
        if buf.trim().is_empty() {
            None
        } else {
            Some(buf)
        }
    } else {
        None
    };

    let out = crate::tools::ctx_pack::handle(
        "pr",
        project_root,
        base.as_deref(),
        format.as_deref(),
        depth,
        diff.as_deref(),
    );
    println!("{out}");
}

fn cmd_pack_create(args: &[String], project_root: &str) {
    let mut name: Option<String> = None;
    let mut version = "1.0.0".to_string();
    let mut description = String::new();
    let mut author: Option<String> = None;
    let mut tags: Vec<String> = Vec::new();
    let mut layers_str: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "create" {
            i += 1;
            continue;
        }
        if let Some(v) = a.strip_prefix("--name=") {
            name = Some(v.to_string());
        } else if a == "--name" {
            i += 1;
            if let Some(v) = args.get(i).filter(|v| !v.starts_with("--")) {
                name = Some(v.clone());
            }
        } else if let Some(v) = a.strip_prefix("--version=") {
            version = v.to_string();
        } else if a == "--version" {
            i += 1;
            if let Some(v) = args.get(i).filter(|v| !v.starts_with("--")) {
                v.clone_into(&mut version);
            }
        } else if let Some(v) = a.strip_prefix("--description=") {
            description = v.to_string();
        } else if a == "--description" {
            i += 1;
            if let Some(v) = args.get(i).filter(|v| !v.starts_with("--")) {
                v.clone_into(&mut description);
            }
        } else if let Some(v) = a.strip_prefix("--author=") {
            author = Some(v.to_string());
        } else if a == "--author" {
            i += 1;
            if let Some(v) = args.get(i).filter(|v| !v.starts_with("--")) {
                author = Some(v.clone());
            }
        } else if let Some(v) = a.strip_prefix("--tags=") {
            tags = v.split(',').map(|s| s.trim().to_string()).collect();
        } else if let Some(v) = a.strip_prefix("--layers=") {
            layers_str = Some(v.to_string());
        }
        i += 1;
    }

    let Some(pkg_name) = name else {
        eprintln!("ERROR: --name is required for pack create");
        return;
    };

    let requested_layers: Vec<&str> = layers_str.as_deref().map_or_else(
        || vec!["knowledge", "graph", "session", "gotchas"],
        |s| s.split(',').map(str::trim).collect(),
    );

    let mut builder = crate::core::context_package::PackageBuilder::new(&pkg_name, &version)
        .description(&description)
        .tags(tags);

    if let Some(ref a) = author {
        builder = builder.author(a);
    }

    let phash = crate::core::project_hash::hash_project_root(project_root);
    builder = builder.project_hash(&phash);

    if requested_layers.contains(&"knowledge") || requested_layers.contains(&"patterns") {
        builder = builder.add_knowledge_from_project(project_root);
    }
    if requested_layers.contains(&"graph") {
        builder = builder.add_graph_from_project(project_root);
    }
    if requested_layers.contains(&"session") {
        if let Some(session) = crate::core::session::SessionState::load_latest() {
            builder = builder.add_session(&session);
        }
    }
    if requested_layers.contains(&"gotchas") {
        builder = builder.add_gotchas_from_project(project_root);
    }

    match builder.build() {
        Ok((manifest, content)) => {
            let registry = match crate::core::context_package::LocalRegistry::open() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("ERROR: cannot open registry: {e}");
                    return;
                }
            };

            match registry.install(&manifest, &content) {
                Ok(dir) => {
                    println!("Package created successfully:");
                    println!("  Name:    {}", manifest.name);
                    println!("  Version: {}", manifest.version);
                    println!(
                        "  Layers:  {}",
                        manifest
                            .layers
                            .iter()
                            .map(crate::core::context_package::PackageLayer::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    println!("  Stats:");
                    println!("    Knowledge facts: {}", manifest.stats.knowledge_facts);
                    println!("    Graph nodes:     {}", manifest.stats.graph_nodes);
                    println!("    Graph edges:     {}", manifest.stats.graph_edges);
                    println!("    Patterns:        {}", manifest.stats.pattern_count);
                    println!("    Gotchas:         {}", manifest.stats.gotcha_count);
                    println!(
                        "    Compression:     {:.1}%",
                        manifest.stats.compression_ratio * 100.0
                    );
                    println!("  Size:    {} bytes", manifest.integrity.byte_size);
                    println!(
                        "  SHA256:  {}...{}",
                        &manifest.integrity.sha256[..8],
                        &manifest.integrity.sha256[56..]
                    );
                    println!("  Stored:  {}", dir.display());
                }
                Err(e) => eprintln!("ERROR: install failed: {e}"),
            }
        }
        Err(e) => eprintln!("ERROR: build failed: {e}"),
    }
}

fn cmd_pack_install(args: &[String], project_root: &str) {
    let mut pkg_name: Option<String> = None;
    let mut pkg_version: Option<String> = None;
    let mut from_file: Option<String> = None;

    for a in args {
        if a == "install" {
            continue;
        }
        if let Some(v) = a.strip_prefix("--file=") {
            from_file = Some(v.to_string());
        } else if let Some(v) = a.strip_prefix("--version=") {
            pkg_version = Some(v.to_string());
        } else if !a.starts_with("--") && pkg_name.is_none() {
            if a.contains('@') {
                let parts: Vec<&str> = a.splitn(2, '@').collect();
                pkg_name = Some(parts[0].to_string());
                pkg_version = Some(parts[1].to_string());
            } else {
                pkg_name = Some(a.clone());
            }
        }
    }

    if let Some(file_path) = from_file {
        let registry = match crate::core::context_package::LocalRegistry::open() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("ERROR: {e}");
                return;
            }
        };
        match registry.import_from_file(std::path::Path::new(&file_path)) {
            Ok(manifest) => {
                println!("Imported: {} v{}", manifest.name, manifest.version);
                apply_package(&manifest.name, &manifest.version, project_root);
            }
            Err(e) => eprintln!("ERROR: import failed: {e}"),
        }
        return;
    }

    let Some(name) = pkg_name else {
        eprintln!("ERROR: package name is required");
        eprintln!("Usage: lean-ctx pack install <name>[@version] [--file=path]");
        return;
    };

    let registry = match crate::core::context_package::LocalRegistry::open() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ERROR: {e}");
            return;
        }
    };

    let resolved_version;
    let version = if let Some(v) = pkg_version.as_deref() {
        v
    } else {
        resolved_version = registry
            .list()
            .ok()
            .and_then(|entries| {
                entries
                    .iter()
                    .filter(|e| e.name == name)
                    .max_by(|a, b| a.installed_at.cmp(&b.installed_at))
                    .map(|e| e.version.clone())
            })
            .unwrap_or_default();
        &resolved_version
    };

    apply_package(&name, version, project_root);
}

fn apply_package(name: &str, version: &str, project_root: &str) {
    let registry = match crate::core::context_package::LocalRegistry::open() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ERROR: {e}");
            return;
        }
    };

    match registry.load_package(name, version) {
        Ok((manifest, content)) => {
            match crate::core::context_package::load_package(&manifest, &content, project_root) {
                Ok(report) => {
                    println!("{report}");
                    println!("Package applied successfully.");
                }
                Err(e) => eprintln!("ERROR: load failed: {e}"),
            }
        }
        Err(e) => eprintln!("ERROR: {e}"),
    }
}

fn cmd_pack_list() {
    let registry = match crate::core::context_package::LocalRegistry::open() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ERROR: {e}");
            return;
        }
    };

    match registry.list() {
        Ok(entries) => {
            if entries.is_empty() {
                println!("No packages installed.");
                println!("Create one with: lean-ctx pack create --name <name>");
                return;
            }

            let header = format!(
                "{:<24} {:<10} {:<30} {:<10} AUTO-LOAD",
                "NAME", "VERSION", "LAYERS", "SIZE"
            );
            println!("{header}");
            println!("{}", "-".repeat(84));

            for e in &entries {
                println!(
                    "{:<24} {:<10} {:<30} {:<10} {}",
                    e.name,
                    e.version,
                    e.layers.join(", "),
                    format_bytes(e.byte_size),
                    if e.auto_load { "yes" } else { "no" }
                );
            }
            println!("\n{} package(s) installed.", entries.len());
        }
        Err(e) => eprintln!("ERROR: {e}"),
    }
}

fn cmd_pack_info(args: &[String]) {
    let pkg_ref = args.iter().find(|a| !a.starts_with("--") && *a != "info");
    let Some(pkg_ref) = pkg_ref else {
        eprintln!("Usage: lean-ctx pack info <name>[@version]");
        return;
    };

    let (name, version) = if pkg_ref.contains('@') {
        let parts: Vec<&str> = pkg_ref.splitn(2, '@').collect();
        (parts[0], Some(parts[1]))
    } else {
        (pkg_ref.as_str(), None)
    };

    let registry = match crate::core::context_package::LocalRegistry::open() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ERROR: {e}");
            return;
        }
    };

    let resolved_ver;
    let ver = if let Some(v) = version {
        v
    } else {
        resolved_ver = registry
            .list()
            .ok()
            .and_then(|entries| {
                entries
                    .iter()
                    .filter(|e| e.name == name)
                    .max_by(|a, b| a.installed_at.cmp(&b.installed_at))
                    .map(|e| e.version.clone())
            })
            .unwrap_or_default();
        &resolved_ver
    };

    match registry.load_package(name, ver) {
        Ok((manifest, content)) => {
            println!("Package: {} v{}", manifest.name, manifest.version);
            if !manifest.description.is_empty() {
                println!("Description: {}", manifest.description);
            }
            if let Some(ref a) = manifest.author {
                println!("Author: {a}");
            }
            println!(
                "Created: {}",
                manifest.created_at.format("%Y-%m-%d %H:%M UTC")
            );
            println!(
                "Layers: {}",
                manifest
                    .layers
                    .iter()
                    .map(crate::core::context_package::PackageLayer::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if !manifest.tags.is_empty() {
                println!("Tags: {}", manifest.tags.join(", "));
            }
            println!("\nStats:");
            println!("  Knowledge facts:  {}", manifest.stats.knowledge_facts);
            println!("  Graph nodes:      {}", manifest.stats.graph_nodes);
            println!("  Graph edges:      {}", manifest.stats.graph_edges);
            println!("  Patterns:         {}", manifest.stats.pattern_count);
            println!("  Gotchas:          {}", manifest.stats.gotcha_count);
            println!(
                "  Compression:      {:.1}%",
                manifest.stats.compression_ratio * 100.0
            );
            println!("  Est. tokens:      ~{}", content.estimated_token_count());
            println!("\nIntegrity:");
            println!("  SHA256:       {}", manifest.integrity.sha256);
            println!("  Content hash: {}", manifest.integrity.content_hash);
            println!(
                "  Size:         {}",
                format_bytes(manifest.integrity.byte_size)
            );
            println!("\nProvenance:");
            println!(
                "  Tool:    {} v{}",
                manifest.provenance.tool, manifest.provenance.tool_version
            );
            if let Some(ref h) = manifest.provenance.project_hash {
                println!("  Project: {h}");
            }
        }
        Err(e) => eprintln!("ERROR: {e}"),
    }
}

fn cmd_pack_remove(args: &[String]) {
    let pkg_ref = args
        .iter()
        .find(|a| !a.starts_with("--") && *a != "remove" && *a != "rm");

    let Some(pkg_ref) = pkg_ref else {
        eprintln!("Usage: lean-ctx pack remove <name>[@version]");
        return;
    };

    let (name, version) = if pkg_ref.contains('@') {
        let parts: Vec<&str> = pkg_ref.splitn(2, '@').collect();
        (parts[0], Some(parts[1]))
    } else {
        (pkg_ref.as_str(), None)
    };

    let registry = match crate::core::context_package::LocalRegistry::open() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ERROR: {e}");
            return;
        }
    };

    match registry.remove(name, version) {
        Ok(0) => eprintln!("No matching package found: {name}"),
        Ok(n) => println!("Removed {n} package(s)."),
        Err(e) => eprintln!("ERROR: {e}"),
    }
}

fn cmd_pack_export(args: &[String]) {
    let mut pkg_ref: Option<&str> = None;
    let mut output: Option<String> = None;

    for a in args {
        if a == "export" {
            continue;
        }
        if let Some(v) = a.strip_prefix("--output=") {
            output = Some(v.to_string());
        } else if let Some(v) = a.strip_prefix("-o=") {
            output = Some(v.to_string());
        } else if !a.starts_with("--") && pkg_ref.is_none() {
            pkg_ref = Some(a.as_str());
        }
    }

    let Some(pkg_ref) = pkg_ref else {
        eprintln!("Usage: lean-ctx pack export <name>[@version] [--output=path]");
        return;
    };

    let (name, version) = if pkg_ref.contains('@') {
        let parts: Vec<&str> = pkg_ref.splitn(2, '@').collect();
        (parts[0].to_string(), parts[1].to_string())
    } else {
        let registry = match crate::core::context_package::LocalRegistry::open() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("ERROR opening registry: {e}");
                return;
            }
        };
        let ver = registry
            .list()
            .ok()
            .and_then(|entries| {
                entries
                    .iter()
                    .filter(|e| e.name == pkg_ref)
                    .max_by(|a, b| a.installed_at.cmp(&b.installed_at))
                    .map(|e| e.version.clone())
            })
            .unwrap_or_default();
        (pkg_ref.to_string(), ver)
    };

    let out_path =
        output.unwrap_or_else(|| crate::core::contracts::default_package_filename(&name, &version));

    let registry = match crate::core::context_package::LocalRegistry::open() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ERROR: {e}");
            return;
        }
    };

    match registry.export_to_file(&name, &version, &PathBuf::from(&out_path)) {
        Ok(bytes) => {
            println!("Exported: {out_path} ({})", format_bytes(bytes));
        }
        Err(e) => eprintln!("ERROR: {e}"),
    }
}

fn cmd_pack_import(args: &[String], project_root: &str) {
    let file_path = args.iter().find(|a| !a.starts_with("--") && *a != "import");
    let apply = args.iter().any(|a| a == "--apply");

    let Some(file_path) = file_path else {
        eprintln!("Usage: lean-ctx pack import <file.ctxpkg> [--apply]");
        return;
    };

    let registry = match crate::core::context_package::LocalRegistry::open() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ERROR: {e}");
            return;
        }
    };

    match registry.import_from_file(std::path::Path::new(file_path)) {
        Ok(manifest) => {
            println!("Imported: {} v{}", manifest.name, manifest.version);
            println!(
                "  Layers: {}",
                manifest
                    .layers
                    .iter()
                    .map(crate::core::context_package::PackageLayer::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            println!("  Size:   {}", format_bytes(manifest.integrity.byte_size));

            if apply {
                apply_package(&manifest.name, &manifest.version, project_root);
            } else {
                println!("\nTo apply this package to the current project:");
                println!("  lean-ctx pack install {}", manifest.name);
            }
        }
        Err(e) => eprintln!("ERROR: import failed: {e}"),
    }
}

fn cmd_pack_auto_load(args: &[String]) {
    let mut pkg_ref: Option<&str> = None;
    let mut enable = true;

    for a in args {
        if a == "auto-load" {
            continue;
        }
        if a == "--off" || a == "--disable" {
            enable = false;
        } else if !a.starts_with("--") && pkg_ref.is_none() {
            pkg_ref = Some(a.as_str());
        }
    }

    let Some(pkg_ref) = pkg_ref else {
        let registry = match crate::core::context_package::LocalRegistry::open() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("ERROR: {e}");
                return;
            }
        };
        match registry.auto_load_packages() {
            Ok(entries) => {
                if entries.is_empty() {
                    println!("No packages set for auto-load.");
                } else {
                    println!("Auto-load packages:");
                    for e in &entries {
                        println!("  {} v{}", e.name, e.version);
                    }
                }
            }
            Err(e) => eprintln!("ERROR: {e}"),
        }
        return;
    };

    let (name, version) = if pkg_ref.contains('@') {
        let parts: Vec<&str> = pkg_ref.splitn(2, '@').collect();
        (parts[0], parts[1].to_string())
    } else {
        let Ok(registry) = crate::core::context_package::LocalRegistry::open() else {
            eprintln!("Failed to open package registry");
            return;
        };
        let ver = registry
            .list()
            .ok()
            .and_then(|entries| {
                entries
                    .iter()
                    .filter(|e| e.name == pkg_ref)
                    .max_by(|a, b| a.installed_at.cmp(&b.installed_at))
                    .map(|e| e.version.clone())
            })
            .unwrap_or_default();
        (pkg_ref, ver)
    };

    let registry = match crate::core::context_package::LocalRegistry::open() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ERROR: {e}");
            return;
        }
    };

    match registry.set_auto_load(name, &version, enable) {
        Ok(()) => {
            if enable {
                println!("Auto-load enabled for {name}@{version}");
            } else {
                println!("Auto-load disabled for {name}@{version}");
            }
        }
        Err(e) => eprintln!("ERROR: {e}"),
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn cmd_pack_send(args: &[String], project_root: &str) {
    use crate::core::a2a_transport::{
        serialize_envelope, AgentIdentityV1, TransportContentType, TransportEnvelopeV1,
    };

    let file: Option<String> = args
        .iter()
        .find(|a| crate::core::contracts::is_package_file(std::path::Path::new(a.as_str())))
        .cloned();
    let target_url = parse_flag(args, "--target");
    let recipient = parse_flag(args, "--to");
    let secret = parse_flag(args, "--secret");

    let Some(f) = file else {
        eprintln!(
            "Usage: lean-ctx pack send <file.{ext}> [--target <url>] [--to <agent>] [--secret <key>]",
            ext = crate::core::contracts::PACKAGE_EXTENSION
        );
        return;
    };
    let pkg_file = PathBuf::from(f);

    let content = match std::fs::read_to_string(&pkg_file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error reading {}: {e}", pkg_file.display());
            return;
        }
    };

    let sender = AgentIdentityV1::from_current("cli", "lean-ctx-cli");
    let mut envelope = TransportEnvelopeV1::new(
        sender,
        recipient.as_deref(),
        TransportContentType::ContextPackage,
        content,
    );
    envelope
        .metadata
        .insert("source_file".to_string(), pkg_file.display().to_string());

    {
        use sha2::{Digest, Sha256};
        let hash = format!("{:x}", Sha256::digest(project_root.as_bytes()));
        envelope
            .metadata
            .insert("project_root_hash".to_string(), hash[..16].to_string());
    }

    if let Some(ref s) = secret {
        envelope.sign(s.as_bytes());
    }

    let json = match serialize_envelope(&envelope) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("Error serializing envelope: {e}");
            return;
        }
    };

    if let Some(ref url) = target_url {
        let endpoint = format!("{}/v1/a2a/handoff", url.trim_end_matches('/'));
        let body = json.as_bytes().to_vec();
        match ureq::post(&endpoint)
            .header("Content-Type", "application/json")
            .send(&body)
        {
            Ok(resp) => {
                let status = resp.status();
                if (200..300).contains(&status.as_u16()) {
                    eprintln!("Sent to {endpoint} — HTTP {status}");
                } else {
                    eprintln!("ERROR: server returned HTTP {status} for {endpoint}");
                }
            }
            Err(e) => eprintln!("Send failed: {e}"),
        }
    } else {
        let out_path = pkg_file.with_extension(format!(
            "{}.envelope.json",
            crate::core::contracts::PACKAGE_EXTENSION
        ));
        match std::fs::write(&out_path, &json) {
            Ok(()) => eprintln!("Envelope written: {}", out_path.display()),
            Err(e) => eprintln!("Write failed: {e}"),
        }
    }
}

fn cmd_pack_receive(args: &[String], project_root: &str) {
    use crate::core::a2a_transport::{parse_envelope, TransportContentType};

    let file: Option<String> = args
        .iter()
        .find(|a| {
            let p = std::path::Path::new(a.as_str());
            p.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "json" || crate::core::contracts::is_package_file(p))
        })
        .cloned();
    let secret = parse_flag(args, "--secret");
    let apply = args.iter().any(|a| a == "--apply");

    let Some(f) = file else {
        eprintln!("Usage: lean-ctx pack receive <envelope.json> [--secret <key>] [--apply]");
        return;
    };
    let envelope_file = PathBuf::from(f);

    let json = match std::fs::read_to_string(&envelope_file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error reading {}: {e}", envelope_file.display());
            return;
        }
    };

    let envelope = match parse_envelope(&json) {
        Ok(env) => env,
        Err(e) => {
            eprintln!("Error parsing envelope: {e}");
            return;
        }
    };

    if let Some(ref s) = secret {
        if !envelope.verify_signature(s.as_bytes()) {
            eprintln!("ERROR: Signature verification failed. Envelope may be tampered.");
            return;
        }
        eprintln!("Signature verified.");
    } else if envelope.signature.is_some() {
        eprintln!("WARNING: Envelope is signed but no --secret provided. Skipping verification.");
    }

    eprintln!(
        "Received from: {} ({})",
        envelope.sender.agent_id, envelope.sender.agent_type
    );
    eprintln!("Content type: {:?}", envelope.content_type);
    eprintln!("Payload size: {} bytes", envelope.payload_json.len());

    match envelope.content_type {
        TransportContentType::ContextPackage => {
            let tmp = std::env::temp_dir().join(format!(
                "lean-ctx-received-{}.{}",
                std::process::id(),
                crate::core::contracts::PACKAGE_EXTENSION
            ));
            if let Err(e) = std::fs::write(&tmp, &envelope.payload_json) {
                eprintln!("Error writing temp file: {e}");
                return;
            }
            if apply {
                let registry = match crate::core::context_package::LocalRegistry::open() {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("ERROR: {e}");
                        return;
                    }
                };
                match registry.import_from_file(&tmp) {
                    Ok(manifest) => {
                        eprintln!("Imported: {} v{}", manifest.name, manifest.version);
                        apply_package(&manifest.name, &manifest.version, project_root);
                    }
                    Err(e) => eprintln!("ERROR: import failed: {e}"),
                }
            } else {
                eprintln!("Package saved to {}. Use --apply to import.", tmp.display());
            }
        }
        TransportContentType::HandoffBundle => {
            let out_path = std::path::Path::new(project_root)
                .join(".lean-ctx")
                .join("handoffs")
                .join("received-bundle.json");
            if let Some(parent) = out_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&out_path, &envelope.payload_json) {
                Ok(()) => eprintln!("Handoff bundle saved: {}", out_path.display()),
                Err(e) => eprintln!("Write failed: {e}"),
            }
        }
        _ => {
            eprintln!(
                "Content type {:?} — payload printed to stdout.",
                envelope.content_type
            );
            println!("{}", envelope.payload_json);
        }
    }
}

/// Parse `--flag=value` or `--flag value` from args.
fn parse_flag(args: &[String], flag: &str) -> Option<String> {
    let prefix = format!("{flag}=");
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if let Some(v) = a.strip_prefix(&prefix) {
            return Some(v.to_string());
        }
        if a == flag {
            if let Some(next) = iter.next() {
                if !next.starts_with("--") {
                    return Some(next.clone());
                }
            }
        }
    }
    None
}

fn print_usage() {
    let ext = crate::core::contracts::PACKAGE_EXTENSION;
    eprintln!(
        "lean-ctx pack — Context Package Manager\n\n\
         SUBCOMMANDS:\n\
         \n\
         Create & Manage:\n\
         \x20 create   --name <name> [--version <v>] [--description <d>] [--author <a>] [--tags <t>] [--layers <l>]\n\
         \x20 list     List all installed packages\n\
         \x20 info     <name>[@version]  Show package details\n\
         \x20 remove   <name>[@version]  Remove a package\n\
         \n\
         Share & Distribute:\n\
         \x20 export   <name>[@version] [--output=<path>]  Export to .{ext} file\n\
         \x20 import   <file.{ext}> [--apply]            Import from file\n\
         \x20 install  <name>[@version] [--file=<path>]    Apply package to current project\n\
         \n\
         A2A Transport:\n\
         \x20 send     <file.{ext}> [--target <url>] [--to <agent>] [--secret <key>]\n\
         \x20 receive  <envelope.json> [--secret <key>] [--apply]\n\
         \n\
         Automation:\n\
         \x20 auto-load [<name>[@version]] [--off]          Manage auto-load packages\n\
         \n\
         PR Pack:\n\
         \x20 pr       [--base <ref>] [--format json|markdown] [--depth <n>]  PR context pack\n\
         \n\
         EXAMPLES:\n\
         \x20 lean-ctx pack create --name rust-patterns --description \"Rust best practices\"\n\
         \x20 lean-ctx pack export rust-patterns --output=rust-patterns.{ext}\n\
         \x20 lean-ctx pack send rust-patterns.{ext} --target http://remote:3344\n\
         \x20 lean-ctx pack receive envelope.json --secret mykey --apply\n\
         \x20 lean-ctx pack list\n"
    );
}
