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
use std::collections::{ HashMap, HashSet };
use std::iter::FromIterator;

use std::process::exit;

#[macro_use]
extern crate lazy_static;

use kmp::{ kmp_find_with_lsp_table, kmp_table };

use clap::{clap_app, crate_version};
use log::*;

const DEFAULT_PROFILE: isize = 0;
const ANY_PROFILE: isize = -1;

#[derive(Debug, Eq, PartialEq)]
enum Command{ Serve, Delay, After, Reset, Profile, }

#[derive(Debug)]
struct Mock<'a> {
    filenames: &'a str,
    patterns: Vec<&'a str>,
    time: Option<Duration>,
    delay: Option<Duration>,
    profile: isize,
    destination_profile: isize,
    command: Command,
    line_number: usize
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
    let command_line_params = clap_app!(
        ("iron-mockside") => 
        (version: crate_version!())
        (author: "Ovidiu Ionescu <ovidiu@ionescu.net>")
        (about: "A mock server useful for testing")
        (@arg debug: -d +multiple "Set debug level debug information")
        (@arg ("address:port"): +required "Address and port to listen to, e.g. 0.0.0.0:8080")
        (@arg ("config file"): +required "Configuration file, e.g. mocks/config.txt")
    ).get_matches();

    let log_level = command_line_params.occurrences_of("debug") as usize;
    stderrlog::new()
        .module(module_path!())
        .quiet(false)
        .verbosity(log_level)
        .timestamp(stderrlog::Timestamp::Off)
        .init()
        .unwrap();
    
        trace!("{:#?}", command_line_params);
    

    let config_file_name = command_line_params.value_of("config file").unwrap();
    println!("Processing configuration file: {}", config_file_name);
    
    let config_file = read_to_string(config_file_name).unwrap();
    env::set_current_dir(std::path::Path::new(config_file_name).parent().unwrap()).unwrap();
    let config = process_config_file(&config_file).unwrap();
        info!("Parsed configuration:\n{:#?}", config);
    if !verify_response_files_exist(&config) {
        error!("Invalid config file, not all reponse files exist");
        exit(1);
    }
    if !verify_all_profiles_are_referenced(&config) {
        error!("Invalid config file, not all defined profiles are reachable");
        exit(1);
    }
    if !verify_mocks_dont_shadow_each_other(&config) {
        error!("Invalid config file, some mocks are shadowed by previously defined ones and are not reachable");
        exit(1);
    }

    let default_mock = Mock {
        filenames: "404.html",
        patterns: Vec::new(),
        time: None,
        delay: None,
        profile: -1,
        destination_profile: ANY_PROFILE,
        command: Command::Serve,
        line_number: 0,
    };
    let mut time = Instant::now();
    let mut profile = DEFAULT_PROFILE;

    let mut counter: usize = 0;

    let kmp_tables = KmpTables::new();

    let address = command_line_params.value_of("address:port").unwrap();

    println!("Starting server: {}", address);

    let listener = TcpListener::bind(address).unwrap();

    for stream in listener.incoming() {
        let stream = stream.unwrap();
        counter += 1;
        handle_connection(stream, &config, &default_mock, &mut time, &mut profile,counter, &kmp_tables);
    }
}

/**
 * Extract the named group profile from the regex match
 */
fn get_profile(group: regex::Captures, found_profiles: &mut HashMap<String, isize>, profile_counter: &mut isize) -> isize {
    match group.name("profile") {
        Some(m) => {
            let profile = m.as_str();

            match found_profiles.get(profile) {
                Some(id) => *id,
                None =>  {
                    *profile_counter = *profile_counter + 1;
                    found_profiles.insert(String::from(profile), *profile_counter);
                    *profile_counter
                }
            }
        },
        None => DEFAULT_PROFILE
    }
}

fn process_config_file(config_file: &str) -> Result<Vec<Mock>, &'static str> {
    let mut config = Vec::with_capacity(100);
    let mut profile_counter = DEFAULT_PROFILE;
    let mut found_profiles: HashMap<String, isize> = HashMap::default();
    found_profiles.insert(String::from("default"), DEFAULT_PROFILE);
    found_profiles.insert(String::from("any"), ANY_PROFILE);
    'mocks: for (_key, group) in config_file
        .lines()
        .enumerate()
        // eliminate comments
        .filter(|(_line_number, s)| !s.trim_start().starts_with("#")) 
        // use line is empty as criteria for grouping
        .group_by(|(_line_number, s)| s.trim().is_empty()) 
        // iterate over the groups
        .into_iter()
        // remove the groups of empty lines
        .filter(|(key, _group)| !key)
    {
        let mut group_line_number = 0;
        let mut patterns: Vec<&str> = group.enumerate().map(|(n, (line_number, s))| {
            if n == 0 {
                // get the number of the first line in the group
                group_line_number = line_number;
            }
            s.trim()
        }).collect();

        if let Some(filenames) = patterns.pop() {
            {
                // after
                lazy_static! {
                    static ref TIME: Regex = Regex::new(r"(?x)
                        ^`(\s*\[(?P<profile>.+)\]\s+)? # profile name
                        after\s*(?P<time>\d+)\s*;      # after duration
                        ").unwrap();
                }
                if let Some(group) = TIME.captures_iter(filenames).next() {
                    config.push(Mock {
                        filenames,
                        patterns,
                        time: Some(Duration::from_millis(group.name("time").unwrap().as_str().parse().unwrap())),
                        delay: None,
                        profile: get_profile(group, &mut found_profiles, &mut profile_counter),
                        destination_profile: ANY_PROFILE,
                        command: Command::After,
                        line_number: group_line_number,

                    });
                    continue 'mocks;
                };
            }
            {
                // delay
                lazy_static! {
                    static ref DELAY: Regex = Regex::new(r"(?x)
                        ^`(\s*\[(?P<profile>.+)\]\s+)? # profile name
                        delay\s*(?P<delay>\d+)\s*;     # delay duration
                        ").unwrap();
                }
                if let Some(group) = DELAY.captures_iter(filenames).next() {
                    config.push(Mock {
                        filenames,
                        patterns,
                        time: None,
                        delay: Some(Duration::from_millis(group.name("delay").unwrap().as_str().parse().unwrap())),
                        profile: get_profile(group, &mut found_profiles, &mut profile_counter),
                        destination_profile: ANY_PROFILE,
                        command: Command::Delay,
                        line_number: group_line_number,
                    });
                        
                    continue 'mocks;               
                };
            }
            {
                // profile
                lazy_static! {
                    static ref SWITCH_PROFILE: Regex = Regex::new(r"^`\s*profile\s+\[(?P<profile>.+)\]\s*;.+").unwrap();
                }
                if let Some(group) = SWITCH_PROFILE.captures_iter(filenames).next() {
                    config.push(Mock {
                        filenames,
                        patterns,
                        time: None,
                        delay: None,
                        profile: ANY_PROFILE,
                        destination_profile: get_profile(group, &mut found_profiles, &mut profile_counter),
                        command: Command::Profile,
                        line_number: group_line_number,
                    });

                    continue 'mocks;
                }
            }
            {
                // only profile specified
                lazy_static! {
                    static ref PROFILE: Regex = Regex::new(r"^`\s*\[(?P<profile>.+)\]\s*;.+").unwrap();
                }
                if let Some(group) = PROFILE.captures_iter(filenames).next() {
                    config.push(Mock {
                        filenames,
                        patterns,
                        time: None,
                        delay: None,
                        profile: get_profile(group, &mut found_profiles, &mut profile_counter),
                        destination_profile: ANY_PROFILE,
                        command: Command::Serve,
                        line_number: group_line_number,
                    });

                    continue 'mocks;
                }
            }
            {
                // reset
                lazy_static! {
                    static ref RESET: Regex = Regex::new(r"^`\s*reset\s*;.+").unwrap();
                }
                if let Some(_) = RESET.captures_iter(filenames).next() {
                    config.push(Mock {
                        filenames,
                        patterns,
                        time: None,
                        delay: None,
                        profile: DEFAULT_PROFILE,
                        destination_profile: ANY_PROFILE,
                        command: Command::Reset,
                        line_number: group_line_number,
                    });

                    continue 'mocks;
                }
            }

            // if the last line starts with a ` it should have been parsed by now
            if filenames.starts_with('`') {
                let err = "Could not parse instructions";
                eprintln!("{}:", err);
                patterns.push(filenames);
                eprintln!("{:#?}", patterns);
                return Err(err);
            }

            config.push(Mock {
                filenames,
                patterns,
                time: None,
                delay: None,
                profile: DEFAULT_PROFILE,
                destination_profile: ANY_PROFILE,
                command: Command::Serve,
                line_number: group_line_number,
            });
        }
    }
    Ok(config)
}

fn verify_response_files_exist(config: &Vec<Mock>) -> bool {
    info!("Verifying all referenced files exist");
    let mut result = true;
    let mut verified_files: HashSet<&str> = HashSet::default();
    for mock in config {
        let mut filename_iterator = mock.filenames.split(";").map(|s| s.trim()).filter(|s| s.len() > 0);

        if mock.filenames.starts_with('`') {
            filename_iterator.next();
        }
        for file in filename_iterator {
            if verified_files.insert(file) {
                if !Path::new(file).exists() {
                    error!("Could not find file: {}, from mock starting at line {}", file, mock.line_number);
                    info!("{:#?}", mock);
                    result = false;
                }
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
        assert_eq!(super::DEFAULT_PROFILE, config[0].profile);
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

    #[test]
    fn process_config_with_profile_name() {
        let config_file = r##"
        POST /path1
        `[my profile] after 1000;headers;body

        GET /path
        `[second profile];headers;body

        "##;

        let config = super::process_config_file(config_file).unwrap();
        assert_eq!(2, config.len());
        assert_eq!(Some(super::Duration::from_millis(1000)), config.first().unwrap().time);
        assert_eq!(1, config.first().unwrap().profile);
        assert_eq!(2, config.last().unwrap().profile);
    }
}

fn verify_all_profiles_are_referenced(config: &Vec<Mock>) -> bool {
    info!("Verifying all profiles are referenced");

    let mut result = true;
    
    let referenced_profiles: HashSet<isize> = HashSet::from_iter(config.iter().filter(|m| m.command == Command::Profile).map(|m| m.destination_profile).into_iter());
    config.iter()
        .filter(|m| m.profile != DEFAULT_PROFILE && m.profile != ANY_PROFILE)
        .filter(|m| !referenced_profiles.contains(&m.profile))
        .for_each(|m| {
            error!("{} - non default profile not referenced by any profile switch statement, group starting at line {}", m.filenames, m.line_number);
            result = false;
        });
    result
}

#[cfg(test)]
    mod tests_profiles_referenced {
    
    #[test]
    fn process_bad_config_file_with_unreferenced_profile() {
        let config_file = r##"
        POST /path
        `[profile1]; headers
        "##;

        let config = super::process_config_file(config_file).unwrap();
        assert!(!super::verify_all_profiles_are_referenced(&config));
    }

    #[test]
    fn process_config_file_with_unreferenced_default_profile() {
        let config_file = r##"
        GET /default
        headers;default.html
        
        POST /path
        `[profile1]; headers

        /switch
        `profile [profile1]; headers; ok.html
        "##;

        let config = super::process_config_file(config_file).unwrap();
        assert!(super::verify_all_profiles_are_referenced(&config));
    }

}

fn shadowed(head: &Mock, tail: &Mock) -> bool {
    if head.profile != ANY_PROFILE && head.profile != tail.profile {
        return false;
    }
    if head.time != tail.time {
        return false;
    }
    
    None == head.patterns.iter().find(|hp| 
        // a tail pattern that doesn't contain any pattern from the head -> tail mock is not shadowed
        None == tail.patterns.iter().find(|pt| pt.contains(**hp))
    )
}

fn verify_mocks_dont_shadow_each_other(config: &Vec<Mock>) -> bool {
    info!("Verifying mocks don't shadow each other, i.e. all mocks are reachable");

    let mut result = true;
    let mut remaining: &[Mock]  = config;

    let mut safety_count = 0;
    while remaining.len() > 1 {
        safety_count += 1;
        if safety_count > 100000 {
            println!("Remaining to check for shadow {:#?}", remaining);
            panic!("Too many iterations in verify_mocks_dont_shadow_each_other");
        }
        if let Some((head, tail)) = remaining.split_first() {
            tail.iter().filter(|t| shadowed(head, t)).for_each(|t| {
                result = false;
                error!("Criteria at line {}: {:#?} shadows line {} {:#?}, maybe they are in the wrong order?", head.line_number, head.patterns, t.line_number, t.patterns);
                debug!("Criteria {:#?} shadows {:#?}, maybe they are in the wrong order?", head, t)
            });
            remaining = tail;
        }
    }
    result
}

#[cfg(test)]
mod tests_shadowing {
    use std::time::{Duration};
    fn make_mock(patterns: Vec<&str>, profile: isize, time: Option<Duration>) -> super::Mock {
        super::Mock {
            filenames: "",
            patterns,
            time,
            delay: None,
            profile,
            destination_profile: -1,
            command: super::Command::Serve,
            line_number: 0,
        }
    }

    #[test]
    fn shadowed_test() {
        let head = make_mock(vec!["switch"], 0, None);
        let tail = make_mock(vec!["switch_to_default"], 0, None);
        assert!(super::shadowed(&head, &tail));
    }

    #[test]
    fn not_shadowed_test() {
        let head = make_mock(vec!["switch_to_default"], 0, None);
        let tail = make_mock(vec!["switch"], 0, None);
        assert!(!super::shadowed(&head, &tail));
    }    
    #[test]
    fn multiple_not_shadowed_test() {
        let head = make_mock(vec!["switch", "header:value"], 0, None);
        let tail = make_mock(vec!["switch_to_default"], 0, None);
        assert!(!super::shadowed(&head, &tail));
    }

    #[test]
    fn multiple_not_shadowed_test_more_lines() {
        let head = make_mock(vec!["switch", "header:value"], 0, None);
        let tail = make_mock(vec!["switch_to_default"], 0, None);
        assert!(!super::shadowed(&head, &tail));
    }

    #[test]
    fn not_shadowd_if_different_profile() {
        let head = make_mock(vec!["switch"], 0, None);
        let tail = make_mock(vec!["switch"], 1, None);
        assert!(!super::shadowed(&head, &tail));

        let head = make_mock(vec!["switch"], -1, None);
        let tail = make_mock(vec!["switch"], 1, None);
        assert!(super::shadowed(&head, &tail));        
    }
    #[test]
    fn not_shadowd_if_different_time() {
        let head = make_mock(vec!["switch"], 0, Some(Duration::from_millis(1000)));
        let tail = make_mock(vec!["switch"], 0, Some(Duration::from_millis(0)));
        assert!(!super::shadowed(&head, &tail));
    }

    #[test]
    fn not_shadowed_if_only_one_has_time() {
        let head = make_mock(vec!["switch"], 0, Some(Duration::from_millis(1000)));
        let tail = make_mock(vec!["switch"], 0, None);
        assert!(!super::shadowed(&head, &tail));
    }

    #[test]
    fn shadowed_if_same_time() {
        let head = make_mock(vec!["switch"], 0, Some(Duration::from_millis(1000)));
        let tail = make_mock(vec!["switch"], 0, Some(Duration::from_millis(1000)));
        assert!(super::shadowed(&head, &tail));
    }

    #[test]
    fn config_not_shadowing() {
        let config = vec![
            make_mock(vec!["switch"], 0, None),
            make_mock(vec!["switch_to_default"], 0, None),
        ];
        assert!(!super::verify_mocks_dont_shadow_each_other(&config));
    }

    #[test]
    fn config_empty_config_not_shadowing_more_lines() {
        let config = vec![
            make_mock(vec!["switch", "header:value1"], 0, None),
            make_mock(vec!["switch_to_default"], 0, None),
            make_mock(vec!["switch", "header:value2"], 0, None),
        ];
        assert!(super::verify_mocks_dont_shadow_each_other(&config));
    }

    #[test]
    fn config_empty_config_not_shadowing() {
        assert!(super::verify_mocks_dont_shadow_each_other(&vec![]));
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
    profile: &mut isize,
    counter: usize,
    kmp_tables: &KmpTables,
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
    let mock = match find_mock(&request, &config, time_origin, *profile) {
        Some(mock) => { mock_found = true; mock},
        None => default_mock,
    };
    
    if mock_found {
        if counter %2 == 0 {
            // light green
            print!("\x1b[32;1m");
        } else {
            // green
            print!("\x1b[32m");
        }
    } else {
        // "\x1B[31;1;4m" red, bold, underligned
        print!("\x1B[31;1m");
    }
    println!("=========================\nRequest {}:\n{}\n\n", counter, request);
    info!("Current profile {}\n, mock: {:#?}", *profile, mock);
    println!("Response: {}", mock.filenames);
    // Reset the colors
    print!("\x1B[0m");
    match mock.command {
        Command::Reset => *time_origin = Instant::now(),
        Command::Delay => thread::sleep(mock.delay.unwrap()),
        Command::Profile => {
            println!("Switched to profile {} from {}", mock.destination_profile, *profile);
            *profile = mock.destination_profile;
        },
        _ => ()
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
    profile: isize,
) -> Option<&'b Mock<'b>> {
    'outside: for mock in config {
        if mock.profile != ANY_PROFILE && profile != mock.profile && mock.command != Command::Profile {
                continue 'outside;
        }
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
