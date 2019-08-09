use itertools::Itertools;
use std;
use std::env;
use std::fs::{read_to_string, File};
use std::io;
use std::io::prelude::*;
use std::net::{TcpListener, TcpStream};

use std::time::{Instant, Duration};

use regex::Regex;
#[macro_use]
extern crate lazy_static;

struct Mock<'a> {
    name: &'a str,
    filenames: &'a str,
    patterns: Vec<&'a str>,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        println!(
            "Missing parameters\nUsage: {} address:port config_file",
            args[0]
        );
        std::process::exit(1);
    }
    let listener = TcpListener::bind(&args[1]).unwrap();

    let config_file_name = &args[2];
    let config_file = read_to_string(config_file_name).unwrap();
    env::set_current_dir(std::path::Path::new(config_file_name).parent().unwrap()).unwrap();
    let config = process_config_file(&config_file);

    let default_mock = Mock {
        name: "default",
        filenames: "404.html",
        patterns: Vec::new(),
    };
    let mut time = Instant::now();

    for stream in listener.incoming() {
        let stream = stream.unwrap();
        handle_connection(stream, &config, &default_mock, &mut time);
    }
}

fn process_config_file(config_file: &str) -> Vec<Mock> {
    let mut config = Vec::with_capacity(100);
    for (_key, group) in config_file
        .lines()
        .filter(|s| !s.trim_start().starts_with("#"))
        .group_by(|s| s.trim().is_empty())
        .into_iter()
        .filter(|(key, _group)| !key)
    {
        let mut patterns: Vec<&str> = group.map(|s| s.trim()).collect();
        if let Some(filenames) = patterns.pop() {
            config.push(Mock {
                name: "",
                filenames,
                patterns,
            });
        }
    }
    config
}

#[cfg(test)]
mod tests {
    #[test]
    fn process_config_file() {
        let config_file = r##"
            # comment
            POST /path
            headers;body

            GET /path
            headers
            "##;

        let config = super::process_config_file(config_file);
        assert_eq!(2, config.len());
    }
}

fn handle_connection(
    mut stream: TcpStream,
    config: &Vec<Mock>,
    default_mock: &Mock,
    time_origin: &mut Instant,
) {
    let mut buffer = [0; 20480];
    stream.read(&mut buffer).unwrap();
    let request = String::from_utf8_lossy(&buffer[..]);
    println!("=========================\nRequest:\n{}\n\n", request);

    // let mock = find_mock(&request, &config).unwrap_or_else(|| default_mock);
    let mock = match find_mock(&request, &config, time_origin) {
        Some(mock) => mock,
        None => default_mock,
    };

    println!("Response: {}", mock.filenames);
    if mock.filenames.starts_with("`reset") {
        *time_origin = Instant::now();
    }

    let mut filename_iterator = mock.filenames.split(";").map(|s| s.trim());
    if mock.filenames.starts_with('`')  {
        filename_iterator.next();
    }
    for file in filename_iterator {
        let mut from_file = File::open(file).unwrap();
        io::copy(&mut from_file, &mut stream).expect("Failed to copy to socket");
    }
    stream.flush().unwrap();
}

fn find_mock<'a, 'b>(
    request: &'a str,
    config: &'b Vec<Mock>,
    time_origin: &Instant,
) -> Option<&'b Mock<'b>> {
    'outside: for mock in config {
        for pattern in &mock.patterns {
            if !request.contains(pattern) {
                continue 'outside;
            }
        }
        if mock.filenames.starts_with('`') {
            lazy_static! {
                static ref TIME: Regex = Regex::new(r"^`\s*(\d+)\s*;").unwrap();
            }
            if let Some(group) = TIME.captures_iter(mock.filenames).next() {
                let d = Duration::from_millis(group[1].parse().unwrap());
                if Instant::now().duration_since(*time_origin) < d {
                    continue 'outside;
                }
            }
        }

        return Some(mock);
    }
    None
}

#[cfg(test)]
mod tests2 {
    use regex::Regex;
    #[test]
    fn regex_check() {
        let s = r"`5000;aha";
        lazy_static! {
            static ref time_regex: Regex = Regex::new(r"^`\s*(\d+)\s*;").unwrap();
        }
        let mut groups = time_regex.captures_iter(s);
        if let Some(group) = groups.next() {
            let val:u32 = group[1].parse().unwrap();
            println!("time: {}, parsed: {}", &group[1], val);
        }
    }
}
