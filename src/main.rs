#![feature(plugin)]
#![plugin(docopt_macros, regex_macros)]

extern crate docopt;
extern crate rustc_serialize;
extern crate git2;
extern crate regex;
extern crate itertools;
#[macro_use] extern crate log;
extern crate fern;

use std::env;
use std::fmt::Write;

use rustc_serialize::{Decodable, Decoder};
use itertools::Itertools;
use git2::Repository;

docopt! { Args, r"
Usage:
  git-pull-requests [options] <commit-range>
  git-pull-requests --help
  git-pull-requests --version

Options:
  --skip-invalid      Skip invalid merge commits.
  --repo-name <repo>  Set repository name to be used in output.
  --format <format>   Set output format [default: markdown]
  --omit-author       Do not print commit author names.
  --help, -h          Show this message.
  --version           Show application version.
", flag_repo_name: Option<String>, flag_format: OutputFormat }

const VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");

struct Config {
    output_format: OutputFormat,
    repo_name: Option<String>,
    omit_author: bool
}

macro_rules! try_error {
    ($e:expr, $ei:ident => $($args:tt)*) => {
        match $e {
            Ok(r) => r,
            Err($ei) => {
                error!($($args)*);
                return;
            }
        }
    }
}

#[derive(Copy, Clone)]
enum OutputFormat {
    Markdown
}

impl Decodable for OutputFormat {
    fn decode<D: Decoder>(d: &mut D) -> Result<OutputFormat, D::Error> {
        d.read_str().and_then(|s| match &s[..] {
            "markdown" => Ok(OutputFormat::Markdown),
            s => Err(d.error(&format!("unknown format: {}", s)))
        })
    }
}

impl OutputFormat {
    fn format(self, info: &PullRequestInfo, config: &Config) -> String {
        match self {
            OutputFormat::Markdown => {
                let mut r: String = " * ".into();
                if let Some(ref repo) = config.repo_name {
                    r.push_str(repo);
                }
                write!(&mut r, "#{} ", info.id).unwrap();
                if !config.omit_author {
                    write!(&mut r, "(by {}) ", info.author).unwrap();
                }
                write!(&mut r, "- {}", info.name).unwrap();
                r
            }
        }
    }
}

#[derive(Clone, Debug)]
struct PullRequestInfo {
    id: u32,
    author: String,
    branch: String,
    name: String
}

impl PullRequestInfo {
    fn from_commit<'a>(c: git2::Commit<'a>) -> Result<PullRequestInfo, String> {
        let msg = match c.message() {
            Some(msg) => msg,
            None => return Err(format!("cannot get commit message for commit {}", c.id()))
        };

        let (header, body): (Option<String>, String) = {
            let mut lines_iter = msg.lines();
            let header = lines_iter.next().map(|s| s.into());
            let body = lines_iter.join("\n");
            (header, body.trim().into())
        };

        if header.is_none() {
            return Err(format!("merge commit {} has empty message", c.id()));
        }
        let header = header.unwrap();

        let header_pattern = regex!(r"Merge pull request #(\d+) from (.+?)/(.+)");
        let (id, author, branch) = if let Some(captures) = header_pattern.captures(&header) {
            let id = match captures.at(1).unwrap().parse() {
                Ok(id) => id,
                Err(e) => return Err(format!("merge commit {} has invalid pull request id {}: {}", c.id(), captures.at(1).unwrap(), e))
            };
            let author = captures.at(2).unwrap().into();
            let branch = captures.at(3).unwrap().into();
            (id, author, branch)
        } else {
            return Err(format!("merge commit {} has invalid pull request header line: {}", c.id(), header));
        };

        Ok(PullRequestInfo {
            id: id,
            author: author,
            branch: branch,
            name: body
        })
    }
}

fn main() {
    let logger_config = fern::DispatchConfig {
        format: Box::new(|msg, level, _| {
            format!("{}: {}", level, msg)
        }),
        output: vec![fern::OutputConfig::stderr()],
        level: log::LogLevelFilter::Trace
    };
    fern::init_global_logger(logger_config, log::LogLevelFilter::Warn).unwrap();

    let args: Args = Args::docopt()
        .help(true)
        .version(VERSION.map(|v| format!("git-pull-requests {}", v)).or_else(|| Some("git-pull-request unknown version".into())))
        .decode()
        .unwrap_or_else(|e| e.exit());

    let config = Config {
        output_format: args.flag_format,
        repo_name: args.flag_repo_name,
        omit_author: args.flag_omit_author
    };

    let current_dir = try_error!(env::current_dir(), e => "cannot get current directory: {}", e);

    let repo = try_error!(Repository::discover(current_dir), e => "cannot open repository: {}", e);

    let mut revwalk = try_error!(repo.revwalk(), e => "cannot get revwalk: {}", e);

    try_error!(revwalk.push_range(&args.arg_commit_range), e => "error pushing range {}: {}", args.arg_commit_range, e);
    revwalk.set_sorting(git2::SORT_TIME);

    let pull_requests = revwalk
        .map(|oid| repo.find_commit(oid).unwrap())
        .filter(|c| c.parents().len() == 2)  // only merge commits
        .map(PullRequestInfo::from_commit);

    let mut any_errors = false;
    let pull_requests: Vec<PullRequestInfo> = pull_requests.filter_map(|pr| match pr {
        Ok(pr) => Some(pr),
        Err(e) => {
            any_errors = true;
            warn!("Error parsing commit: {}", e);
            None
        }
    }).collect();

    if any_errors {
        if args.flag_skip_invalid {
            warn!("Some commits couldn't be parsed, skipping them");
        } else {
            error!("Some commits couldn't be parsed, aborting");
            return;
        }
    }

    for pr in pull_requests {
        println!("{}", config.output_format.format(&pr, &config));
    }
}
