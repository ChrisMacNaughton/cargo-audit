//! Audit Cargo.lock files for crates containing security vulnerabilities

#![crate_name = "cargo_audit"]
#![crate_type = "bin"]
#![deny(missing_docs, missing_debug_implementations, missing_copy_implementations)]
#![deny(trivial_casts, trivial_numeric_casts)]
#![deny(unsafe_code, unstable_features, unused_import_braces, unused_qualifications)]

mod shell;

extern crate clap;
extern crate isatty;
extern crate rustsec;
#[macro_use]
extern crate serde_json;
extern crate term;

use clap::{App, Arg, SubCommand};
use rustsec::{AdvisoryDatabase, Lockfile};
use rustsec::advisory::Advisory;
use rustsec::error::Error as RustSecError;
use rustsec::lockfile::Package;
use shell::{ColorConfig, Shell};
use std::process::exit;
use term::color::{GREEN, RED, WHITE};

enum OutputFormat {
    Text,
    Json,
}

fn main() {
    let matches = App::new("cargo")
        .subcommand(
            SubCommand::with_name("audit")
                .version(env!("CARGO_PKG_VERSION"))
                .author("Tony Arcieri <bascule@gmail.com>")
                .about("Audit Cargo.lock for crates with security vulnerabilities.")
                .arg_from_usage(
                    "-f, --file=[NAME] 'Cargo lockfile to inspect (default: Cargo.lock)'",
                )
                .arg_from_usage("-u, --url=[URL] 'URL from which to fetch advisory database'")
                .arg(
                    Arg::from_usage("--color=[COLOR] Colored output")
                        .possible_values(&["auto", "always", "never"]),
                )
                .arg(
                    Arg::from_usage("--format=[FORMAT] Output Format")
                        .possible_values(&["text", "json"]),
                ),
        )
        .get_matches();

    let (filename, url, color_config, output_format) =
        if let Some(audit_matches) = matches.subcommand_matches("audit") {
            (
                audit_matches.value_of("file").unwrap_or("Cargo.lock"),
                audit_matches
                    .value_of("url")
                    .unwrap_or(rustsec::ADVISORY_DB_URL),
                audit_matches.value_of("color").unwrap_or("auto"),
                audit_matches.value_of("format").unwrap_or("text"),
            )
        } else {
            panic!("cargo-audit is intended to be invoked as a cargo subcommand");
        };

    let mut shell = shell::create(match color_config {
        "always" => ColorConfig::Always,
        "never" => ColorConfig::Never,
        _ => ColorConfig::Auto,
    });

    let output_format = match output_format {
        "text" => OutputFormat::Text,
        "json" => OutputFormat::Json,
        _ => OutputFormat::Text,
    };

    let lockfile = match Lockfile::load(filename) {
        Ok(lf) => lf,
        Err(RustSecError::IO) => {
            not_found(&mut shell, filename).unwrap();
            exit(1);
        }
        Err(ex) => panic!("Couldn't load {}: {}", filename, ex),
    };

    if let OutputFormat::Text = output_format {
        shell
            .say_status("Fetching", &format!("advisories `{}`", url), GREEN, true)
            .unwrap();
    }

    let advisory_db =
        AdvisoryDatabase::fetch_from_url(url).expect("Couldn't fetch advisory database");
    if let OutputFormat::Text = output_format {
        shell
            .say_status(
                "Scanning",
                &format!(
                    "{} crates for vulnerabilities ({} advisories in database)",
                    lockfile.packages.len(),
                    advisory_db.iter().len()
                ),
                GREEN,
                true,
            )
            .unwrap();
    }

    let vulnerabilities = lockfile.vulnerabilities(&advisory_db);
    if let OutputFormat::Text = output_format {
        if vulnerabilities.is_empty() {
            shell
                .say_status("Success", "No vulnerable packages found", GREEN, true)
                .unwrap();
        } else {
            shell
                .say_status("Warning", "Vulnerable crates found!", RED, true)
                .unwrap();
        }
    }
    match output_format {
        OutputFormat::Text => {
            for vuln in &vulnerabilities {
                display_advisory(&mut shell, vuln.package, vuln.advisory).unwrap();
            }

            if !vulnerabilities.is_empty() {
                vulns_found(&mut shell, vulnerabilities.len()).unwrap();
                exit(1);
            }
        }
        OutputFormat::Json => {
            let vulns: Vec<serde_json::Value> = vulnerabilities
                .iter()
                .map(|vuln| {
                    let advisory = vuln.advisory;
                    json!({
                        // tool	"retire"
                        // message	"3rd party CORS request may execute for jquery"
                        // url	"https://github.com/jquery/jquery/issues/2432"
                        // cve	"CVE-2015-9251"
                        // file	"node_modules/sql.js/gh-pages/documentation/javascript/application.js"
                        // priority	"Medium"
                        "tool": "cargo-audit",
                        "message": advisory.title,
                        "url": advisory.url,
                        "cve": advisory.id,
                        "file": "Cargo.lock",
                        "priority": "Unknown",
                    })
                })
                .collect();
            let json_vulns: serde_json::Value = json!(*vulns);
            if vulnerabilities.is_empty() {
                shell.say(json_vulns, GREEN).unwrap();
            } else {
                shell.say(json_vulns, RED).unwrap();
            }
        }
    }
}

fn not_found(shell: &mut Shell, filename: &str) -> term::Result<()> {
    shell.say_status(
        "error:",
        format!("Couldn't find '{}'!", filename),
        RED,
        false,
    )?;
    shell.say(
        "\nRun \"cargo build\" to generate lockfile before running audit",
        WHITE,
    )?;

    Ok(())
}

fn vulns_found(shell: &mut Shell, vuln_count: usize) -> term::Result<()> {
    if vuln_count == 1 {
        shell.say_status("\nerror:", "1 vulnerability found!", RED, false)?;
    } else {
        shell.say_status(
            "\nerror:",
            format!("{} vulnerabilities found!", vuln_count),
            RED,
            false,
        )?;
    }

    Ok(())
}

fn display_advisory(shell: &mut Shell, package: &Package, advisory: &Advisory) -> term::Result<()> {
    attribute(shell, "\nID", &advisory.id)?;
    attribute(shell, "Crate", &package.name)?;
    attribute(shell, "Version", &package.version.to_string())?;

    if let Some(ref date) = advisory.date {
        attribute(shell, "Date", date)?;
    }

    if let Some(ref url) = advisory.url {
        attribute(shell, "URL", url)?;
    }

    attribute(shell, "Title", &advisory.title)?;

    let mut fixed_versions = String::new();
    let version_count = advisory.patched_versions.len();

    for (i, version) in advisory.patched_versions.iter().enumerate() {
        fixed_versions.push_str(&version.to_string());

        if i < version_count - 1 {
            fixed_versions.push_str(", ");
        }
    }

    attribute(shell, "Solution: upgrade to", &fixed_versions)?;

    Ok(())
}

fn attribute(shell: &mut Shell, name: &str, value: &str) -> term::Result<()> {
    shell.say_status(format!("{}:", name), value, RED, false)
}
