use itertools::Itertools;
use std;
use std::env;
use std::fs::{read_to_string, File};
use std::io;
use std::io::prelude::*;
use std::net::{TcpListener, TcpStream};

use std::time::{Duration, Instant};

use regex::Regex;
#[macro_use]
extern crate lazy_static;

use kmp::{ kmp_find_with_lsp_table, kmp_table };

struct Mock<'a> {
    filenames: &'a str,
    patterns: Vec<&'a str>,
    time: Option<Duration>,
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
    let listener = TcpListener::bind(&args[1]).unwrap();

    let config_file_name = &args[2];
    let config_file = read_to_string(config_file_name).unwrap();
    env::set_current_dir(std::path::Path::new(config_file_name).parent().unwrap()).unwrap();
    let config = process_config_file(&config_file);

    let default_mock = Mock {
        filenames: "404.html",
        patterns: Vec::new(),
        time: None,
    };
    let mut time = Instant::now();

    let mut counter: usize = 0;

    let kmp_tables = KmpTables::new();

    for stream in listener.incoming() {
        let stream = stream.unwrap();
        counter += 1;
        handle_connection(stream, &config, &default_mock, &mut time, counter, &kmp_tables);
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
            let time: Option<Duration> = if filenames.starts_with('`') {
                lazy_static! {
                    static ref TIME: Regex = Regex::new(r"^`\s*(\d+)\s*;").unwrap();
                }
                match TIME.captures_iter(filenames).next() {
                    Some(group) => Some(Duration::from_millis(group[1].parse().unwrap())),
                    None => None,
                }
            } else {
                None
            };

            config.push(Mock {
                filenames,
                patterns,
                time,
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

    let mut filename_iterator = mock.filenames.split(";").map(|s| s.trim());
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
