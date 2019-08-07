use std;
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
    let listener = std::net::TcpListener::bind(&args[1]).unwrap();
    let config_file = std::fs::read_to_string(&args[2]).unwrap();
    let config = process_config_file(&config_file);
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