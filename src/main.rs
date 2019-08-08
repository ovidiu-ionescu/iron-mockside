use std;
use std::env;
use std::io;
use std::io::prelude::*;
use std::fs:: {read_to_string, File};
use std::net::{TcpListener, TcpStream };
use itertools::Itertools;

struct Mock <'a>{
    name: &'a str,
    filenames: &'a str,
    patterns: Vec<&'a str>,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        println!("Missing parameters\nUsage: {} address:port config_file", args[0]);
        std::process::exit(1);
    }
    let listener = TcpListener::bind(&args[1]).unwrap();

    let config_file_name = &args[2];
    let config_file = read_to_string(config_file_name).unwrap();
    env::set_current_dir(std::path::Path::new(config_file_name).parent().unwrap()).unwrap();
    let config = process_config_file(&config_file);

    let default_mock = Mock { name: "default", filenames: "404.html", patterns: Vec::new()};
   for stream in listener.incoming() {
        let stream = stream.unwrap();
        handle_connection(stream, &config, &default_mock);
   }
}

fn process_config_file(config_file: &str) -> Vec<Mock> {
    let mut config = Vec::with_capacity(100);
    for (_key, group) in config_file
                            .lines()
                            .filter(|s| !s.trim_start().starts_with("#"))
                            .group_by(|s| s.trim().is_empty())
                            .into_iter()
                            .filter(|(key, _group)| !key ) {
        let mut patterns: Vec<&str> = group.map(|s| s.trim()).collect();
        if let Some(filenames) = patterns.pop() {
            config.push(Mock {
                name: "",
                filenames,
                patterns
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

fn handle_connection(mut stream: TcpStream, config: &Vec<Mock>, default_mock: &Mock) {
    let mut buffer = [0; 20480];
    stream.read(&mut buffer).unwrap();
    let request = String::from_utf8_lossy(&buffer[..]);
    println!("=========================\nRequest:\n{}\n\n", request);

    // let mock = find_mock(&request, &config).unwrap_or_else(|| default_mock);
    let mock = match find_mock(&request, &config) {
        Some(mock) => mock,
        None => default_mock
    };

    println!("Response: {}", mock.filenames);
    for file in mock.filenames.split(";").map(|s| s.trim()) {
        let mut from_file = File::open(file).unwrap();
        io::copy(&mut from_file, &mut stream).expect("Failed to copy to socket");
    }
    stream.flush().unwrap();
}

fn find_mock<'a, 'b>(request: &'a str, config: &'b Vec<Mock>) -> Option<&'b Mock<'b>> {
    'outside: for mock in config {
        for pattern in &mock.patterns {
            if !request.contains(pattern) {
                continue 'outside;
            }
        }
        return Some(mock)
    }
    None
}