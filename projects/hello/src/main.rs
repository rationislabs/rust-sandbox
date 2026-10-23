//! The smallest project: copy this folder to start a new one.

fn greeting(name: &str) -> String {
    format!("Hello, {name}!")
}

fn main() {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "world".to_owned());
    println!("{}", greeting(&name));
}

#[cfg(test)]
mod tests {
    use super::greeting;

    #[test]
    fn greets_by_name() {
        assert_eq!(greeting("Rust"), "Hello, Rust!");
    }
}
