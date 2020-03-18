use itertools::Itertools;
use std;
use std::env;
use std::fs::{read_to_string, File};
use std::io;
use std::io::prelude::*;
use std::net::{TcpListener, TcpStream};

use std::time::{Duration, Instant};
use std::thread;
use std::path::Path;

use regex::Regex;
#[macro_use]
extern crate lazy_static;

use kmp::{ kmp_find_with_lsp_table, kmp_table };

#[derive(Debug)]
struct Mock<'a> {
    filenames: &'a str,
    patterns: Vec<&'a str>,
    time: Option<Duration>,
    delay: Option<Duration>,
}

// to accelerate the search of headers
struct KmpTables {
    content_length_zero: &'static [u8],
    content_length_zero_lsp: Vec<usize>,
    expect_100_continue: &'static [u8],
    expect_100_continue_lsp: Vec<usize>
}

impl KmpTables {
    fn new() -> KmpTables {
        let content_length_zero = b"Content-Length: 0";
        let content_length_zero_lsp = kmp_table(content_length_zero);
        let expect_100_continue = b"Expect: 100-continue";
        let expect_100_continue_lsp = kmp_table(expect_100_continue);
        KmpTables {
            content_length_zero,
            content_length_zero_lsp,
            expect_100_continue,
            expect_100_continue_lsp
        }
    }
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

    let config_file_name = &args[2];
    println!("Processing configuration file: {}", config_file_name);
    
    let config_file = read_to_string(config_file_name).unwrap();
    env::set_current_dir(std::path::Path::new(config_file_name).parent().unwrap()).unwrap();
    let config = process_config_file(&config_file).unwrap();
    if !verify_response_files_exist(&config) {
        panic!("Invalid config file");
    }

    let default_mock = Mock {
        filenames: "404.html",
        patterns: Vec::new(),
        time: None,
        delay: None,
    };
    let mut time = Instant::now();

    let mut counter: usize = 0;

    let kmp_tables = KmpTables::new();

    println!("Starting server: {}", &args[1]);
    let listener = TcpListener::bind(&args[1]).unwrap();

    for stream in listener.incoming() {
        let stream = stream.unwrap();
        counter += 1;
        handle_connection(stream, &config, &default_mock, &mut time, counter, &kmp_tables);
    }
}

fn process_config_file(config_file: &str) -> Result<Vec<Mock>, &'static str> {
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
            
            let time: Option<Duration> = if filenames.starts_with("`after") {
                lazy_static! {
                    static ref TIME: Regex = Regex::new(r"^`after\s*(\d+)\s*;").unwrap();
                }
                match TIME.captures_iter(filenames).next() {
                    Some(group) => Some(Duration::from_millis(group[1].parse().unwrap())),
                    None => {
                        let err = "After line has no number of millis specified";
                        eprintln!("{}: {}", err, filenames);
                        return Err(err);
                    }
                }
            } else {
                None
            };

            let delay: Option<Duration> = if filenames.starts_with("`delay") {
                lazy_static! {
                    static ref DELAY: Regex = Regex::new(r"^`delay\s*(\d+)\s*;").unwrap();
                }
                match DELAY.captures_iter(filenames).next() {
                    Some(group) => Some(Duration::from_millis(group[1].parse().unwrap())),
                    None => {
                        let err = "delay line has no number or millis specified";
                        eprintln!("{}: {}", err, filenames);
                        return Err(err);
                    }
                }
            } else {
                None
            };

            // if the last line stars with ` make sure it was parsed it correctly
            if filenames.starts_with('`') && !filenames.starts_with("`reset") && time.is_none() && delay.is_none() {
                let err = "Could not parse time instructions";
                eprintln!("{}:", err);
                patterns.push(filenames);
                eprintln!("{:#?}", patterns);
                return Err(err);
            }

            config.push(Mock {
                filenames,
                patterns,
                time,
                delay,
            });
        }
    }
    Ok(config)
}

fn verify_response_files_exist(config: &Vec<Mock>) -> bool {
    let mut result = true;
    for mock in config {
        let mut filename_iterator = mock.filenames.split(";").map(|s| s.trim()).filter(|s| s.len() > 0);

        if mock.filenames.starts_with('`') {
            filename_iterator.next();
        }
        for file in filename_iterator {
            if !Path::new(file).exists() {
                result = false;
                eprintln!("Could not find file: {}", file);
                println!("{:#?}", mock);
            }
        }
    }
    result
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

        let config = super::process_config_file(config_file).unwrap();
        assert_eq!(2, config.len());
    }

    #[test]
    fn process_config_file_with_time() {
        let config_file = r##"
        POST /path1
        `after 1000;headers;body

        POST /path2
        `delay 2000;headers;body
        "##;

        let config = super::process_config_file(config_file).unwrap();
        assert_eq!(2, config.len());
        assert_eq!(Some(super::Duration::from_millis(1000)), config.first().unwrap().time);
        assert_eq!(Some(super::Duration::from_millis(2000)), config.last().unwrap().delay);
    }

    #[test]
    fn process_bad_config_file() {
        let config_file = r##"
        POST /path
        `delay; headers
        "##;

        let config = super::process_config_file(config_file);
        assert!(config.is_err());
    }

    #[test]
    fn process_bad_config_file2() {
        let config_file = r##"
        POST /path
        `1000; headers
        "##;

        let config = super::process_config_file(config_file);
        assert!(config.is_err());
    }
}

// check for two consecutive EOL (\n)
fn find_empty_line(buffer: &[u8]) -> bool {
    let mut count = 0;
    let mut found = false;
    match buffer.iter()
        .filter(|&&b| b != b'\r')
        .find(|&&b| {
            match b {
                b'\n' => { 
                    count += 1;
                    if count > 2 {
                        found = true;
                    }
                }
                _ => if count > 1 { 
                    found = true 
                } else { count = 0 },
            }
            found
        }) {
        Some(_found) => true,
        _ => false,
    }
}
#[cfg(test)]
mod empty_line_tests {
    #[test]
    fn has_empty_line() {
        let complete = b"head\r\n\r\nbody";
        assert!(super::find_empty_line(&complete[..]));
        let headers = b"head";
        assert!(!super::find_empty_line(&headers[..]));
        let headers_end = b"head\r\n\r\n";
        assert!(!super::find_empty_line(&headers_end[..]));
        let complete_empty = b"head\r\n\r\n\r\n";
        assert!(super::find_empty_line(&complete_empty[..]));
    }
}

fn handle_connection(
    mut stream: TcpStream,
    config: &Vec<Mock>,
    default_mock: &Mock,
    time_origin: &mut Instant,
    counter: usize,
    kmp_tables: &KmpTables
) {
    let mut buffer = [0; 20480];
    let start = Instant::now();
    println!("Incoming connection");
    let count = stream.read(&mut buffer).unwrap();
    // check if the whole request has been sent
    if &buffer[0..4] == b"POST" {
        let empty_line = find_empty_line(&buffer[..count]);
        if !empty_line {
            if kmp_find_with_lsp_table(kmp_tables.content_length_zero, &buffer[..count], &kmp_tables.content_length_zero_lsp).is_some() {
                println!("Content length is zero");
            } else {
                if kmp_find_with_lsp_table(kmp_tables.expect_100_continue, &buffer[..count], &kmp_tables.expect_100_continue_lsp).is_some() {
                    println!("Send a continue response {}ms", start.elapsed().as_millis());
                    stream.write(b"HTTP/1.1 100 Continue\r\n\r\n").unwrap();
                }
                // read the rest of the body
                println!("Wait for the rest of the body");
                stream.read(&mut buffer[count ..]).unwrap();
                println!("Got the rest of the body {}ms", start.elapsed().as_millis());
            }
        }
    }
    let request = String::from_utf8_lossy(&buffer[..]);

    // let mock = find_mock(&request, &config).unwrap_or_else(|| default_mock);
    let mut mock_found = false;
    let mock = match find_mock(&request, &config, time_origin) {
        Some(mock) => { mock_found = true; mock},
        None => default_mock,
    };
    
    if mock_found {
        if counter %2 == 0 {
            print!("\x1b[32;1m");
        } else {
            print!("\x1b[32m");
        }
    } else {
        // "\x1B[31;1;4m" red, bold, underligned
        print!("\x1B[31;1m");
    }
    println!("=========================\nRequest {}:\n{}\n\n", counter, request);

    println!("Response: {}", mock.filenames);
    // Reset the colors
    print!("\x1B[0m");
    if mock.filenames.starts_with("`reset") {
        *time_origin = Instant::now();
    }

    if let Some(delay) = mock.delay {
        thread::sleep(delay);
    }

    let mut filename_iterator = mock.filenames.split(";").map(|s| s.trim()).filter(|s| s.len() > 0);
    if mock.filenames.starts_with('`') {
        filename_iterator.next();
    }
    for file in filename_iterator {
        let mut from_file = File::open(file).unwrap();
        io::copy(&mut from_file, &mut stream).expect("Failed to copy to socket");
    }
    stream.flush().unwrap();
}

/// Finds a mock in the configuration corresponding to this request
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
        if let Some(duration) = mock.time {
            if Instant::now().duration_since(*time_origin) < duration {
                continue 'outside;
            }
        }

        return Some(mock);
    }
    None
}
