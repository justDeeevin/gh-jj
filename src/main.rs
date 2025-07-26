use clap::{Parser, Subcommand};
use color_eyre::{
    Result,
    eyre::{Context, ContextCompat, eyre},
};
use octocrab::Octocrab;
use regex::Regex;
use std::{convert::Infallible, path::PathBuf, process::Stdio, str::FromStr};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
/// Work with GitHub repositories using the Jujutsu VCS
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand)]
enum CliCommand {
    Clone(CloneCommand),
}

#[derive(Parser)]
/// Clone a repository from GitHub and initialize it as a Jujutsu repo
struct CloneCommand {
    #[arg(value_name = "REPOSITORY")]
    /// Repository to clone
    ///
    /// Uses the same syntax as `gh repo clone`
    repo: String,

    #[arg()]
    /// Directory into which to clone the repository
    ///
    /// By default, a new directory will be created in CWD with the name of the repo
    directory: Option<PathBuf>,

    #[arg(long)]
    /// Colocate the repository (place `.git` in the root of the repository)
    colocate: bool,

    #[arg(short, long, default_value = "upstream")]
    /// Upstream remote name when cloning a fork
    upstream_remote_name: String,

    #[arg(allow_hyphen_values = true, num_args = 0.., last = true)]
    /// Arguments to pass to `jj git clone`
    rest: Vec<String>,
}

enum Source {
    GitHub { owner: Option<String>, repo: String },
    Web(String),
}

impl FromStr for Source {
    type Err = Infallible;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let regex = Regex::new(r"^(?:([a-zA-Z0-9-]+)\/)?([a-zA-Z0-9_.-]+)$").unwrap();
        Ok(if let Some(captures) = regex.captures(s) {
            Source::GitHub {
                owner: captures.get(1).map(|m| m.as_str().to_string()),
                repo: captures.get(2).unwrap().as_str().to_string(),
            }
        } else {
            Source::Web(s.to_string())
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();

    match cli.command {
        CliCommand::Clone(cmd) => clone(cmd).await,
    }
}

async fn clone(cmd: CloneCommand) -> Result<()> {
    let source: Source = cmd.repo.parse().unwrap();
    let mut upstream = None;

    let repo_url = match source {
        Source::GitHub {
            owner,
            repo: repo_name,
        } => {
            use gh_config::GITHUB_COM;

            let hosts = gh_config::Hosts::load().context("Failed to get gh hosts")?;

            let owner = match owner {
                Some(owner) => owner,
                None => hosts
                    .get(GITHUB_COM)
                    .context("No github.com host found")?
                    .user
                    .clone()
                    .context("No github.com user found")?,
            };

            let token = hosts
                .retrieve_token(GITHUB_COM)
                .context("Failed to retrieve github token")?
                .context("No github.com token found")?;

            let octocrab = Octocrab::builder().personal_token(token).build()?;

            let repo = octocrab
                .repos(&owner, &repo_name)
                .get()
                .await
                .context("Failed to get repo info")?;

            upstream = repo.parent;

            format!("https://github.com/{owner}/{repo_name}")
        }
        Source::Web(url) => url,
    };

    let mut command = Command::new("jj");
    command.arg("git").arg("clone");
    if cmd.colocate {
        command.arg("--colocate");
    }
    command.arg(repo_url);
    if let Some(directory) = &cmd.directory {
        command.arg(directory);
    }

    command.args(cmd.rest);

    let mut jj_clone = command
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn jj")?;

    let mut reader =
        BufReader::new(jj_clone.stderr.take().context("Failed to take jj stderr")?).lines();
    let mut extracted_directory = None;
    let extract_dir_regex = Regex::new(r#"^Fetching into new repo in "(.*)"$"#).unwrap();

    while let Some(line) = reader
        .next_line()
        .await
        .context("Failed to read jj output")?
    {
        eprintln!("{line}");
        if extracted_directory.is_none() {
            let Some(captures) = extract_dir_regex.captures(line.trim()) else {
                continue;
            };
            extracted_directory = Some(PathBuf::from(captures.get(1).unwrap().as_str()));
        }
    }

    if !jj_clone
        .wait()
        .await
        .context("Failed to execute jj cli")?
        .success()
    {
        return Err(eyre!("JJ clone failed"));
    }

    let Some(upstream) = upstream else {
        return Ok(());
    };

    let directory = cmd
        .directory
        .or(extracted_directory)
        .context("Couldn't find directory")?;

    if !Command::new("jj")
        .arg("-R")
        .arg(directory)
        .arg("git")
        .arg("remote")
        .arg("add")
        .arg(cmd.upstream_remote_name)
        .arg(upstream.url.as_str())
        .status()
        .await
        .context("Failed to execute jj cli")?
        .success()
    {
        return Err(eyre!("Failed to add upstream remote"));
    }

    Ok(())
}
