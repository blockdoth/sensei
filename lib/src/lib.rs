pub fn get_message() -> String {
    "Hello Nix flake!".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn get_message_test() {
        assert_eq!(get_message(), "Hello Nix flake!");
    }
}
