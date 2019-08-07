use itertools::Itertools;

fn main() {
    let str = "aha\nbib\n\n\n\ndex\ngig\n\n";

    for (_key, group) in str.lines().group_by(|s| s.trim().is_empty()).into_iter().filter(|(key, _group)| !key ) {
        println!("Group:");
        for s in group {
            println!("{}", s);
        }
        println!("");
    }
}
