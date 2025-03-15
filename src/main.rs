extern crate clap;

use std::str::FromStr;

use ::http::header::HeaderName;
use ::http::{HeaderMap, HeaderValue, Method};
use anyhow::{Context, Error, Result};
use clap::{crate_version, Arg, ArgAction, ArgMatches, Command};
use hyper::body::Bytes;
use mimalloc::MiMalloc;
use regex::Regex;
use rustls::crypto::ring::default_provider;
use tokio::time::Duration;

mod bench;
mod http;
mod results;
mod runtime;
mod utils;

use crate::http::BenchType;

/// Matches a string like '12d 24h 5m 45s' to a regex capture.
static DURATION_MATCH: &str =
  "(?P<days>[0-9]+)d|(?P<hours>[0-9]+)h|(?P<minutes>[0-9]+)m|(?P<seconds>[0-9]+)s";

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// ReWrk
///
/// Captures CLI arguments and build benchmarking settings and runtime to
/// suite the arguments and options.
fn main() {
  let args = parse_args();

  match default_provider().install_default() {
    Ok(_) => (),
    Err(_) => {
      eprintln!("cannot install Rustls crypto provider.");
      return;
    }
  }

  let threads: usize = match args
    .get_one::<String>("threads")
    .unwrap_or(&"1".to_string())
    .trim()
    .parse()
  {
    Ok(v) => v,
    Err(_) => {
      eprintln!("invalid parameter for 'threads' given, input type must be a integer.");
      return;
    }
  };

  let conns: usize = match args
    .get_one::<String>("connections")
    .unwrap_or(&"1".to_string())
    .trim()
    .parse()
  {
    Ok(v) => v,
    Err(_) => {
      eprintln!("invalid parameter for 'connections' given, input type must be a integer.");
      return;
    }
  };

  let host: &str = match args.get_one::<String>("host") {
    Some(v) => v,
    None => {
      eprintln!("missing 'host' parameter.");
      return;
    }
  };

  let http2: bool = args.get_flag("http2");
  let json: bool = args.get_flag("json");

  let bench_type = if http2 {
    BenchType::HTTP2
  } else {
    BenchType::HTTP1
  };

  let default_duration = "1s".to_string();
  let duration: &str = args
    .get_one::<String>("duration")
    .unwrap_or(&default_duration);
  let duration = match parse_duration(duration) {
    Ok(dur) => dur,
    Err(e) => {
      eprintln!("failed to parse duration parameter: {}", e);
      return;
    }
  };

  let pct: bool = args.get_flag("pct");

  let rounds: usize = args
    .get_one::<String>("rounds")
    .unwrap_or(&"1".to_string())
    .trim()
    .parse::<usize>()
    .unwrap_or(1);

  let method = match args
    .get_one::<String>("method")
    .map(|method| Method::from_str(&method.to_uppercase()))
    .transpose()
  {
    Ok(method) => method.unwrap_or(Method::GET),
    Err(e) => {
      eprintln!("failed to parse method: {}", e);
      return;
    }
  };

  let headers = if let Some(headers) = args.get_many::<String>("header") {
    match headers
      .map(|s| s as &str)
      .map(parse_header)
      .collect::<Result<HeaderMap<_>>>()
    {
      Ok(headers) => headers,
      Err(e) => {
        eprintln!("failed to parse header: {}", e);
        return;
      }
    }
  } else {
    HeaderMap::new()
  };

  let empty_body = "".to_string();
  let body: &String = args.get_one::<String>("body").unwrap_or(&empty_body);
  let body = Bytes::copy_from_slice(body.as_bytes());

  let settings = bench::BenchmarkSettings {
    threads,
    connections: conns,
    host: host.to_string(),
    bench_type,
    duration,
    display_percentile: pct,
    display_json: json,
    rounds,
    method,
    headers,
    body,
  };

  bench::start_benchmark(settings);
}

/// Parses a duration string from the CLI to a Duration.
/// '11d 3h 32m 4s' -> Duration
///
/// If no matches are found for the string or a invalid match
/// is captured a error message returned and displayed.
fn parse_duration(duration: &str) -> Result<Duration> {
  let mut dur = Duration::default();

  let re = Regex::new(DURATION_MATCH).unwrap();
  for cap in re.captures_iter(duration) {
    let add_to = if let Some(days) = cap.name("days") {
      let days = days.as_str().parse::<u64>()?;

      let seconds = days * 24 * 60 * 60;
      Duration::from_secs(seconds)
    } else if let Some(hours) = cap.name("hours") {
      let hours = hours.as_str().parse::<u64>()?;

      let seconds = hours * 60 * 60;
      Duration::from_secs(seconds)
    } else if let Some(minutes) = cap.name("minutes") {
      let minutes = minutes.as_str().parse::<u64>()?;

      let seconds = minutes * 60;
      Duration::from_secs(seconds)
    } else if let Some(seconds) = cap.name("seconds") {
      let seconds = seconds.as_str().parse::<u64>()?;

      Duration::from_secs(seconds)
    } else {
      return Err(Error::msg(format!("invalid match: {:?}", cap)));
    };

    dur += add_to
  }

  if dur.as_secs() == 0 {
    return Err(Error::msg(format!(
      "failed to extract any valid duration from {}",
      duration
    )));
  }

  Ok(dur)
}

fn parse_header(value: &str) -> Result<(HeaderName, HeaderValue)> {
  let (key, value) = value
    .split_once(": ")
    .context("Header value missing colon (\": \")")?;
  let key = HeaderName::from_str(key).context("Invalid header name")?;
  let value = HeaderValue::from_str(value).context("Invalid header value")?;
  Ok((key, value))
}

/// Contains Clap's app setup.
fn parse_args() -> ArgMatches {
  Command::new("FerrBench")
    .version(crate_version!())
    .about("Benchmark HTTP/1 and HTTP/2 frameworks without pipelining bias.")
    .disable_help_flag(true)
    .arg(
      Arg::new("help")
        .long("help")
        .action(ArgAction::Help)
        .help("Print help"),
    )
    .arg(
      Arg::new("threads")
        .short('t')
        .long("threads")
        .help("Set the amount of threads to use e.g. '-t 12'")
        .action(ArgAction::Set)
        .default_value("1"),
    )
    .arg(
      Arg::new("connections")
        .short('c')
        .long("connections")
        .help("Set the amount of concurrent e.g. '-c 512'")
        .action(ArgAction::Set)
        .default_value("1"),
    )
    .arg(
      Arg::new("host")
        .short('h')
        .long("host")
        .help("Set the host to bench e.g. '-h http://127.0.0.1:5050'")
        .action(ArgAction::Set)
        .required(true),
    )
    .arg(
      Arg::new("http2")
        .long("http2")
        .help("Set the client to use http2 only. (default is http/1) e.g. '--http2'")
        .required(false)
        .action(ArgAction::SetTrue),
    )
    .arg(
      Arg::new("duration")
        .short('d')
        .long("duration")
        .help("Set the duration of the benchmark.")
        .action(ArgAction::Set)
        .required(true),
    )
    .arg(
      Arg::new("pct")
        .long("pct")
        .help("Displays the percentile table after benchmarking.")
        .action(ArgAction::SetTrue)
        .required(false),
    )
    .arg(
      Arg::new("json")
        .long("json")
        .help("Displays the results in a json format")
        .action(ArgAction::SetTrue)
        .required(false),
    )
    .arg(
      Arg::new("rounds")
        .long("rounds")
        .short('r')
        .help("Repeats the benchmarks n amount of times")
        .action(ArgAction::Set)
        .required(false),
    )
    .arg(
      Arg::new("method")
        .long("method")
        .short('m')
        .help("Set request method e.g. '-m get'")
        .action(ArgAction::Append)
        .required(false),
    )
    .arg(
      Arg::new("header")
        .long("header")
        .short('H')
        .help("Add header to request e.g. '-H \"content-type: text/plain\"'")
        .action(ArgAction::Append)
        .required(false),
    )
    .arg(
      Arg::new("body")
        .long("body")
        .short('b')
        .help("Add body to request e.g. '-b \"foo\"'")
        .action(ArgAction::Set)
        .required(false),
    )
    //.arg(
    //    Arg::new("random")
    //        .long("rand")
    //        .help(
    //            "Sets the benchmark type to random mode, \
    //             clients will randomly connect and re-connect.\n\
    //             NOTE: This will cause the HTTP2 flag to be ignored."
    //        )
    //        .action(ArgAction::Count)
    //        .required(false)
    //)
    .get_matches()
}
