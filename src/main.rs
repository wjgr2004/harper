use base64::{prelude::BASE64_STANDARD_NO_PAD, Engine};
use chrono::{DateTime, Local};
use clap::{Parser, Subcommand};
use hickory_resolver::proto::rr::RecordType;
use hickory_resolver::Resolver;
use std::{
    cmp::Reverse,
    collections::HashMap,
    fs,
    io::{self, IsTerminal, Read},
    process::exit,
};

mod ops;
use ops::{count_requests, count_schemes, count_urls, filter, list_domains, search_for};
use tldextract::TldOption;

#[derive(Parser, Debug)]
#[command(version, about = "Command line HAR analyser.", long_about = None)]
struct Args {
    #[arg(short, long, help = "Filters out requests after the time.", default_value = None, global = true)]
    before: Option<DateTime<Local>>,

    #[arg(short, long, help = "Filters out requests before the time.", default_value = None, global = true)]
    after: Option<DateTime<Local>>,

    #[clap(subcommand)]
    command: Commands,

    /// The HAR file to analyse.
    file: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Count number of times a request is sent to a URL.
    CountUrls(CountUrlArgs),

    /// Count number of each scheme in the HAR.
    CountSchemes,

    /// Count the number of requests made.
    CountRequests,

    /// Search for a specific string.
    SearchFor(SearchForArgs),

    /// Return the contents of the HAR.
    Output,

    /// Check if urls are using DNSSEC
    DNSSECAudit,
}

#[derive(Debug, clap::Args)]
struct CountUrlArgs {
    #[arg(short, long, help="Method used for sorting, sorting is done at each level of the domain tree.", default_value = SortBy::Frequency.as_ref())]
    sort: SortBy,

    #[arg(
        short,
        long,
        help = "Merge the tld and the sld, i.e. merge example and .com"
    )]
    merge_tld: bool,
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum SortBy {
    /// Sort alphanumerically at each level.
    Alpha,

    /// Sort by frequency at each level.
    Frequency,
}

impl AsRef<str> for SortBy {
    fn as_ref(&self) -> &str {
        match self {
            SortBy::Frequency => "frequency",
            SortBy::Alpha => "alpha",
        }
    }
}

#[derive(Debug, clap::Args)]
struct SearchForArgs {
    /// The string to search for.
    string: String,
}

fn main() {
    let args = Args::parse();

    let mut contents;

    if let Some(file) = args.file {
        contents = fs::read_to_string(file).expect("Could not read file at {file_name}.");
    } else {
        let mut stdin = io::stdin();

        if stdin.is_terminal() {
            println!("Please provide a file.");
            exit(2);
        }

        contents = String::new();
        stdin
            .read_to_string(&mut contents)
            .expect("Couldn't read from stdin.");
    }

    let mut parsed = json::parse(&contents).expect("Could not parse file as json.");

    if let Some(dt) = args.before {
        filter::filter_by_time(&mut parsed, dt, false).expect("Invalid HAR file.");
    }

    if let Some(dt) = args.after {
        filter::filter_by_time(&mut parsed, dt, true).expect("Invalid HAR file.");
    }

    match args.command {
        Commands::CountUrls(count_args) => {
            let tld_extractor = TldOption::default()
                .cache_path(".tld_cache")
                .private_domains(false)
                .update_local(false)
                .naive_mode(false)
                .build();

            let mut domain_tree = count_urls::DomainNode::default();
            count_urls::build_domain_tree(
                &parsed,
                &mut domain_tree,
                &tld_extractor,
                count_args.merge_tld,
            );

            match count_args.sort {
                SortBy::Alpha => {
                    count_urls::print_tree(&domain_tree, &mut |(name, _)| name.to_string());
                }
                SortBy::Frequency => {
                    count_urls::print_tree(&domain_tree, &mut |(_, node)| Reverse(node.count));
                }
            }
        }

        Commands::CountSchemes => {
            let mut counts = HashMap::new();
            count_schemes::get_counts(&parsed, &mut counts);

            let mut counts_vec: Vec<(&String, &usize)> = counts.iter().collect();
            counts_vec.sort_by_key(|a| Reverse(a.1));

            for (scheme, count) in counts_vec {
                println!("{}: {}", scheme, count);
            }
        }

        Commands::CountRequests => {
            let count = count_requests::get_counts(&parsed);

            println!("Found {} requests.", count);
        }

        Commands::SearchFor(search_args) => {
            let matches = search_for::search_for(&parsed, &search_args.string);
            for result in matches {
                println!("Found in request {}:", result.request_num);
                println!(
                    "Time: {}\nURL: {}\nMethod: {}\nIn fields: {:?}\n",
                    result.time, result.url, result.method, result.in_fields
                );
            }

            let b64_search_string = BASE64_STANDARD_NO_PAD.encode(&search_args.string);
            let matches_b64 = search_for::search_for(&parsed, &b64_search_string);
            for result in matches_b64 {
                println!("Found base64 encoded in request {}:", result.request_num);
                println!(
                    "Time: {}\nURL: {}\nMethod: {}\nIn fields: {:?}\n",
                    result.time, result.url, result.method, result.in_fields
                );
            }
        }

        Commands::Output => {
            println!("{}", json::stringify_pretty(parsed, 4));
        }

        Commands::DNSSECAudit => {
            let mut domains: Vec<String> = list_domains::list_domains(&parsed);
            domains.sort_by_key(|x| x.chars().rev().collect::<String>());

            let resolver = Resolver::default().unwrap();

            for domain in domains {
                let resp = resolver.lookup(domain.clone() + ".", RecordType::ANY);
                let Ok(resp) = resp else {
                    println!("{}: DNS Lookup Failed", domain);
                    continue;
                };

                let mut sig_found = false;

                for record in resp.records() {
                    sig_found |= record.record_type() == RecordType::RRSIG;
                }

                if sig_found {
                    println!("DNSSEC Signature found for {}", domain)
                } else {
                    println!("{} Doesn't seem to use DNSSEC", domain)
                }
            }
        }
    }
}
