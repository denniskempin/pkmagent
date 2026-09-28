//! Filename sanitizing. A name is chosen only when a file is created.

pub fn allocate_filename(
    display_title: &str,
    empty_title_fallback: &str,
    stable_id: &str,
    taken: &[String],
) -> String {
    let title = sanitize(display_title, empty_title_fallback, 80);
    let plain = fit(&title, "");
    let plain_name = format!("{plain}.md");
    if !taken_name(&plain_name, taken) {
        return plain_name;
    }
    let suffix = sanitize(stable_id, "untitled", 40);
    for n in 0..10_000u32 {
        let middle = if n == 0 {
            format!("--{suffix}")
        } else {
            format!("--{suffix}-{}", n + 1)
        };
        let fitted = fit(&title, &middle);
        let name = format!("{fitted}{middle}.md");
        if name.len() <= 200 && !taken_name(&name, taken) {
            return name;
        }
    }
    format!("{plain}--{suffix}.md")
}

pub fn sanitize(input: &str, empty_fallback: &str, max_scalars: usize) -> String {
    let collapsed = collapse_whitespace(input);
    let source = if collapsed.is_empty() {
        empty_fallback
    } else {
        collapsed.as_str()
    };
    let mut cleaned: String = source
        .chars()
        .map(|ch| {
            if matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
                || ch.is_ascii_control()
            {
                '-'
            } else {
                ch
            }
        })
        .collect();
    while cleaned.ends_with(' ') || cleaned.ends_with('.') {
        cleaned.pop();
    }
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        cleaned = "untitled".to_string();
    }
    cleaned.chars().take(max_scalars).collect()
}

fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn fit(title: &str, middle: &str) -> String {
    let mut fitted = title.to_string();
    while format!("{fitted}{middle}.md").len() > 200 {
        if fitted.pop().is_none() {
            break;
        }
    }
    if fitted.is_empty() {
        "untitled".to_string()
    } else {
        fitted
    }
}

fn taken_name(name: &str, taken: &[String]) -> bool {
    taken.iter().any(|existing| same_name(existing, name))
}

fn same_name(left: &str, right: &str) -> bool {
    left.chars()
        .flat_map(char::to_lowercase)
        .eq(right.chars().flat_map(char::to_lowercase))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_titles_and_falls_back() {
        assert_eq!(
            allocate_filename("  Hello   world \n", "untitled", "id", &[]),
            "Hello world.md"
        );
        assert_eq!(
            allocate_filename("a/b:c*d?e\"f<g>h|i", "untitled", "id", &[]),
            "a-b-c-d-e-f-g-h-i.md"
        );
        assert_eq!(
            allocate_filename("Hello...", "untitled", "id", &[]),
            "Hello.md"
        );
        assert_eq!(
            allocate_filename("..", "untitled", "id", &[]),
            "untitled.md"
        );
        assert_eq!(
            allocate_filename("", "(no subject)", "id", &[]),
            "(no subject).md"
        );
        assert_eq!(
            allocate_filename(" \t\n", "untitled", "id", &[]),
            "untitled.md"
        );
        let control = format!("A{}B", '\u{0001}');
        assert_eq!(allocate_filename(&control, "untitled", "id", &[]), "A-B.md");
    }

    #[test]
    fn collisions_are_case_insensitive_and_use_the_stable_id() {
        let first = allocate_filename("Hello", "untitled", "id/a", &[]);
        assert_eq!(first, "Hello.md");
        let second = allocate_filename("hello", "untitled", "id/b", &[first.clone()]);
        assert_eq!(second, "hello--id-b.md");
        let third = allocate_filename("Hello", "untitled", "id/b", &[first, second]);
        assert_eq!(third, "Hello--id-b-2.md");
        let long_id = "x".repeat(50);
        let name = allocate_filename("Title", "untitled", &long_id, &["Title.md".into()]);
        let suffix = name
            .strip_suffix(".md")
            .unwrap()
            .split("--")
            .nth(1)
            .unwrap();
        assert_eq!(suffix.chars().count(), 40);
    }

    #[test]
    fn caps_scalars_and_utf8_bytes() {
        let long = "a".repeat(100);
        assert_eq!(
            allocate_filename(&long, "untitled", "id", &[]),
            format!("{}.md", "a".repeat(80))
        );
        let wide = "你".repeat(80);
        let name = allocate_filename(&wide, "untitled", "id", &[]);
        assert!(name.len() <= 200, "{}", name.len());
        assert!(name.ends_with(".md"));
        assert!(name.strip_suffix(".md").unwrap().chars().count() < 80);
    }
}
