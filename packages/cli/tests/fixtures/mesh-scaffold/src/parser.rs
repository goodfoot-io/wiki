// Stub: CLI parser.
pub fn parse_args(input: &str) -> Vec<String> {
    input.split_whitespace().map(String::from).collect()
}
