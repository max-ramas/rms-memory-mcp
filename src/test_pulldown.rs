use pulldown_cmark::{Parser, Options, Event, Tag};

pub fn test_parse() {
    let markdown_input = "Hello world, this is a ~~complicated~~ *very simple* example.\n\n```rust\nfn main() {}\n```";
    let parser = Parser::new_ext(markdown_input, Options::all());
    for (event, range) in parser.into_offset_iter() {
        println!("{:?}: {:?}", event, &markdown_input[range]);
    }
}
