//! The shape real values share, as a regular expression.
//!
//! A field declared `format: uuid` whose events all carry
//! `1677831-ff2a5b8e8c8aa859` has two possible repairs, and the obvious one is
//! the worse of them: dropping the format leaves a plain string that accepts
//! anything at all, including the empty string and a sentence. The producers
//! are sending something with a shape; this works that shape out so the
//! contract can say what it is.
//!
//! Deliberately conservative. Every sampled value has to have the same
//! structure — the same runs, the same separators in the same places — or
//! there is no shared shape and nothing is offered. A pattern guessed from
//! values that disagree would reject traffic that is fine today.

/// What a run of characters is made of: any mix of the three.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Classes {
    digit: bool,
    lower: bool,
    upper: bool,
}

impl Classes {
    fn of(c: char) -> Option<Self> {
        match c {
            '0'..='9' => Some(Classes { digit: true, ..Default::default() }),
            'a'..='z' => Some(Classes { lower: true, ..Default::default() }),
            'A'..='Z' => Some(Classes { upper: true, ..Default::default() }),
            _ => None,
        }
    }

    fn widen(&mut self, other: Classes) {
        self.digit |= other.digit;
        self.lower |= other.lower;
        self.upper |= other.upper;
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// A run of one or more characters of these classes.
    Run {
        classes: Classes,
        /// Whether this run reads as hex: digits and letters mixed, no letter
        /// past `f`. A run of letters alone is a word, not an id.
        hex: bool,
        min: usize,
        max: usize,
    },
    /// Anything else, kept literally: a hyphen, a colon, a slash.
    Separator(char),
}

/// Most tokens a pattern is worth writing.
///
/// A value made of twenty alternating runs is not a shape, it is a sentence,
/// and pinning it would be a constraint nobody could read or maintain.
const MAX_TOKENS: usize = 12;

/// A character that could be part of a lowercase hex string.
fn is_hex_letter_or_digit(c: char) -> bool {
    c.is_ascii_digit() || ('a'..='f').contains(&c)
}

/// Split a value into runs and separators.
fn tokenize(value: &str) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    for c in value.chars() {
        match Classes::of(c) {
            None => tokens.push(Token::Separator(c)),
            Some(class) => match tokens.last_mut() {
                // A run continues while the characters stay alphanumeric, so
                // `ff2a5b` is one run rather than three.
                Some(Token::Run {
                    classes,
                    hex,
                    min,
                    max,
                }) => {
                    classes.widen(class);
                    *hex = *hex && is_hex_letter_or_digit(c);
                    *min += 1;
                    *max += 1;
                }
                _ => tokens.push(Token::Run {
                    classes: class,
                    hex: is_hex_letter_or_digit(c),
                    min: 1,
                    max: 1,
                }),
            },
        }
    }
    tokens
}

/// Fold a second value's tokens into the first's, or fail if they disagree.
fn merge(into: &mut [Token], other: Vec<Token>) -> bool {
    if into.len() != other.len() {
        return false;
    }
    for (held, next) in into.iter_mut().zip(other) {
        match (held, next) {
            (Token::Separator(a), Token::Separator(b)) if *a == b => {}
            (
                Token::Run {
                    classes,
                    hex,
                    min,
                    max,
                },
                Token::Run {
                    classes: other_classes,
                    hex: other_hex,
                    min: other_min,
                    max: other_max,
                },
            ) => {
                classes.widen(other_classes);
                *hex = *hex && other_hex;
                *min = (*min).min(other_min);
                *max = (*max).max(other_max);
            }
            _ => return false,
        }
    }
    true
}

/// The character class for a run, narrowest first.
fn render_class(classes: Classes, hex: bool) -> String {
    let Classes { digit, lower, upper } = classes;

    // `[0-9a-f]` rather than `[0-9a-z]` for a run that mixes digits with
    // letters no later than `f`: an id written in hex is the common case, and
    // the narrower class is the one that would catch a wrong value. Letters
    // alone are a word — `abc` is not hex just because it could be.
    if hex && digit && lower && !upper {
        return "[0-9a-f]".to_string();
    }
    match (digit, lower, upper) {
        (true, false, false) => "\\d".to_string(),
        (false, true, false) => "[a-z]".to_string(),
        (false, false, true) => "[A-Z]".to_string(),
        (false, true, true) => "[A-Za-z]".to_string(),
        (true, true, false) => "[0-9a-z]".to_string(),
        (true, false, true) => "[0-9A-Z]".to_string(),
        _ => "[0-9A-Za-z]".to_string(),
    }
}

/// Escape a separator so it means itself, and only where it has to be.
///
/// A pattern is read by whoever maintains the schema, and `\-` where `-` would
/// do is noise in something already hard enough to read.
fn escape(c: char) -> String {
    const META: [char; 14] = [
        '.', '^', '$', '*', '+', '?', '(', ')', '[', ']', '{', '}', '|', '\\',
    ];
    if META.contains(&c) {
        format!("\\{c}")
    } else {
        c.to_string()
    }
}

/// The pattern every one of `values` matches, or nothing when they have no
/// shape in common.
///
/// Needs more than one distinct value: a pattern derived from a single string
/// is that string, and declaring it would reject the second one.
pub fn common_pattern(values: &[&str]) -> Option<String> {
    // A set beside the list: every string of a large sample comes through
    // here, and a scan per value made that quadratic.
    let mut seen = std::collections::HashSet::new();
    let mut distinct: Vec<&str> = Vec::new();
    for value in values {
        if value.is_empty() {
            return None;
        }
        if seen.insert(*value) {
            distinct.push(value);
        }
    }
    if distinct.len() < 2 {
        return None;
    }

    let mut shape = tokenize(distinct[0]);
    if shape.is_empty() || shape.len() > MAX_TOKENS {
        return None;
    }
    for value in &distinct[1..] {
        if !merge(&mut shape, tokenize(value)) {
            return None;
        }
    }

    let mut out = String::from("^");
    for token in &shape {
        match token {
            Token::Separator(c) => out.push_str(&escape(*c)),
            Token::Run {
                classes,
                hex,
                min,
                max,
            } => {
                out.push_str(&render_class(*classes, *hex));
                match (min, max) {
                    (1, 1) => {}
                    (a, b) if a == b => out.push_str(&format!("{{{a}}}")),
                    (a, b) => out.push_str(&format!("{{{a},{b}}}")),
                }
            }
        }
    }
    out.push('$');
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_shape_of_an_id_the_producers_actually_send() {
        // The field that prompted this: declared `format: uuid`, and every
        // event carries something that is plainly structured and plainly not
        // a uuid.
        let pattern = common_pattern(&[
            "1677831-ff2a5b8e8c8aa859",
            "1677832-0a1b2c3d4e5f6789",
            "1677901-deadbeefcafe1234",
        ])
        .unwrap();
        assert_eq!(pattern, r"^\d{7}-[0-9a-f]{16}$");
    }

    #[test]
    fn a_run_that_varies_in_length_says_so() {
        let pattern = common_pattern(&["ab-1", "abc-22", "abcd-333"]).unwrap();
        assert_eq!(pattern, r"^[a-z]{2,4}-\d{1,3}$");
    }

    #[test]
    fn letters_past_f_are_not_called_hex() {
        let pattern = common_pattern(&["zebra", "tiger"]).unwrap();
        assert_eq!(pattern, r"^[a-z]{5}$");
    }

    #[test]
    fn mixed_case_and_digits_widen_rather_than_guess() {
        let pattern = common_pattern(&["AB12cd", "XY99zz"]).unwrap();
        assert_eq!(pattern, r"^[0-9A-Za-z]{6}$");
    }

    #[test]
    fn values_with_different_structure_have_no_shared_shape() {
        // Separators in different places, and one with none at all: any
        // pattern covering both would have to be loose enough to be useless.
        assert_eq!(common_pattern(&["ab-12", "ab12"]), None);
        assert_eq!(common_pattern(&["a-b-c", "a-b"]), None);
        assert_eq!(common_pattern(&["x:1", "x-1"]), None);
    }

    #[test]
    fn one_value_is_not_a_shape() {
        // It would be that exact string, which the next event disproves.
        assert_eq!(common_pattern(&["1677831-ff2a5b8e8c8aa859"]), None);
        assert_eq!(common_pattern(&["same", "same", "same"]), None);
    }

    #[test]
    fn an_empty_value_means_there_is_nothing_to_pin() {
        assert_eq!(common_pattern(&["", "abc"]), None);
    }

    #[test]
    fn separators_are_escaped_so_they_mean_themselves() {
        let pattern = common_pattern(&["a.b/1", "c.d/2"]).unwrap();
        // `.` has to be escaped; `/` does not, and escaping it would only
        // make the pattern harder to read.
        assert_eq!(pattern, r"^[a-z]\.[a-z]/\d$");
    }

    #[test]
    fn a_value_with_no_shape_left_to_describe_is_refused() {
        // Past the token cap: a pattern for this would be longer than the
        // value and read by nobody.
        let long = "a-b-c-d-e-f-g-h-i-j-k-l-m";
        let other = "n-o-p-q-r-s-t-u-v-w-x-y-z";
        assert_eq!(common_pattern(&[long, other]), None);
    }
}
