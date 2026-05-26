use std::collections::{HashMap, HashSet};

use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::{tag, take_while1},
    character::complete::{alphanumeric1, line_ending, not_line_ending, space0, space1},
    combinator::map,
    multi::many0,
    sequence::preceded,
};

#[derive(Debug, PartialEq, Clone)]
pub enum MetadataSegment<'a> {
    Description(&'a str),
    Param { name: &'a str, value: &'a str },
    Hashtag(&'a str),
}

#[derive(Debug, PartialEq)]
pub struct Metadata<'a> {
    pub description: Vec<&'a str>,
    pub hashtags: HashSet<&'a str>,
    pub params: HashMap<&'a str, &'a str>,
}

// Parse a line that starts with "### " followed by text (description)
fn parse_description(input: &'_ str) -> IResult<&'_ str, MetadataSegment<'_>> {
    map(take_while1(|c: char| c != '@' && c != '#' && c != '\r' && c != '\n'), |content: &str| {
        MetadataSegment::Description(content.trim())
    })
    .parse(input)
}

// Parse a line that starts with "### @param " followed by name and value
fn parse_param(input: &'_ str) -> IResult<&'_ str, MetadataSegment<'_>> {
    map(
        (tag("@param"), space1, take_while1(|c: char| !c.is_whitespace()), space1, not_line_ending),
        |(_, _, name, _, value): (&str, &str, &str, &str, &str)| MetadataSegment::Param {
            name,
            value: value.trim(),
        },
    )
    .parse(input)
}

// Parse a line that starts with "### #" followed by hashtag
fn parse_hashtag(input: &'_ str) -> IResult<&'_ str, MetadataSegment<'_>> {
    map(preceded(tag("#"), alphanumeric1), |content: &str| MetadataSegment::Hashtag(content.trim()))
        .parse(input)
}

// Parse any of the comment line types
fn parse_comment_line(input: &'_ str) -> IResult<&'_ str, Vec<MetadataSegment<'_>>> {
    let (input, (_, _, comment, _)) = (
        tag("###"),
        space0,
        many0(alt((parse_param, parse_hashtag, parse_description))),
        line_ending,
    )
        .parse(input)?;

    Ok((input, comment))
}

// Parse the entire comment block
pub fn metadata(input: &str) -> IResult<&str, Metadata<'_>> {
    map(many0(parse_comment_line), |lines| {
        let mut description = Vec::new();
        let mut params = HashMap::new();
        let mut hashtags = HashSet::new();
        lines.iter().flatten().for_each(|line| {
            match *line {
                MetadataSegment::Description(desc) => description.push(desc),
                MetadataSegment::Param { name, value } => {
                    params.insert(name, value);
                },
                MetadataSegment::Hashtag(tag) => {
                    hashtags.insert(tag);
                },
            };
        });
        Metadata {
            description,
            hashtags,
            params,
        }
    })
    .parse(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn print_comment_block(blk: Metadata) {
        for tag in blk.hashtags {
            println!("#{}", tag)
        }
        for (key, val) in blk.params {
            println!("{} = {}", key, val)
        }
        for desc in blk.description {
            println!("> {}", desc)
        }
    }

    #[test]
    fn test_main() {
        let input = r#"### something to describe request
### the description is on multiple lines
### @param host http://localhost:80
### #hashtag
"#;

        let result = metadata(input);
        assert!(result.is_ok());

        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "");

        print_comment_block(parsed);
    }

    #[test]
    fn test_parse_example() {
        let input = r#"### something to describe request
### @param host http://localhost:80
### #hashtag
"#;

        let result = metadata(input);
        assert!(result.is_ok());

        let (remaining, parsed) = result.unwrap();
        assert_eq!(remaining, "");

        print_comment_block(parsed);
    }

    #[test]
    fn test_individual_parsers() {
        // Test description
        let (_, desc) = metadata("### some description\n").unwrap();
        assert_eq!(desc.description.first(), Some(&"some description"));

        // Test param
        let (_, param) = parse_param("@param name value here").unwrap();
        assert_eq!(
            param,
            MetadataSegment::Param {
                name: "name",
                value: "value here"
            }
        );

        // Test hashtag
        let (_, hashtag) = parse_hashtag("#mytag").unwrap();
        assert_eq!(hashtag, MetadataSegment::Hashtag("mytag"));
    }

    #[test]
    fn test_mixed_content() {
        let input = r#"### First description
### @param url https://example.com
### #tag1
### Another description
### @param method GET
### #tag2
"#;

        let (_, parsed) = metadata(input).unwrap();

        match parsed.description.first() {
            Some(desc) => {
                assert_eq!(*desc, "First description")
            },
            _ => panic!("Expected description"),
        }
    }
}
