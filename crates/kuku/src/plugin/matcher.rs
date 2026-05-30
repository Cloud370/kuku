use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum MatcherExpr {
    Cmp {
        var: String,
        op: CmpOp,
        value: String,
    },
    And(Box<MatcherExpr>, Box<MatcherExpr>),
    Or(Box<MatcherExpr>, Box<MatcherExpr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CmpOp {
    Eq,
    Ne,
    Contains,
}

struct Parser {
    input: Vec<char>,
    pos: usize,
}

pub fn parse(input: &str) -> Result<MatcherExpr, String> {
    let mut parser = Parser {
        input: input.chars().collect(),
        pos: 0,
    };
    let expr = parser.parse_or()?;
    parser.skip_whitespace();
    if parser.pos < parser.input.len() {
        return Err(format!(
            "unexpected character '{}' at position {}",
            parser.input[parser.pos], parser.pos
        ));
    }
    Ok(expr)
}

impl Parser {
    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() && self.input[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.input.get(self.pos).copied();
        if ch.is_some() {
            self.pos += 1;
        }
        ch
    }

    fn expect(&mut self, ch: char) -> Result<(), String> {
        self.skip_whitespace();
        match self.advance() {
            Some(c) if c == ch => Ok(()),
            Some(c) => Err(format!(
                "expected '{ch}' but found '{c}' at position {}",
                self.pos - 1
            )),
            None => Err(format!("expected '{ch}' but found end of input")),
        }
    }

    fn parse_or(&mut self) -> Result<MatcherExpr, String> {
        let mut left = self.parse_and()?;
        loop {
            self.skip_whitespace();
            if self.pos + 1 < self.input.len()
                && self.input[self.pos] == '|'
                && self.input[self.pos + 1] == '|'
            {
                self.pos += 2;
                let right = self.parse_and()?;
                left = MatcherExpr::Or(Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<MatcherExpr, String> {
        let mut left = self.parse_atom()?;
        loop {
            self.skip_whitespace();
            if self.pos + 1 < self.input.len()
                && self.input[self.pos] == '&'
                && self.input[self.pos + 1] == '&'
            {
                self.pos += 2;
                let right = self.parse_atom()?;
                left = MatcherExpr::And(Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_atom(&mut self) -> Result<MatcherExpr, String> {
        self.skip_whitespace();
        if self.peek() == Some('(') {
            self.advance();
            let expr = self.parse_or()?;
            self.expect(')')?;
            return Ok(expr);
        }

        let var = self.parse_ident()?;
        self.skip_whitespace();
        let op = self.parse_cmp_op()?;
        self.skip_whitespace();
        let value = self.parse_string()?;

        Ok(MatcherExpr::Cmp { var, op, value })
    }

    fn parse_ident(&mut self) -> Result<String, String> {
        self.skip_whitespace();
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(format!("expected identifier at position {start}"));
        }
        Ok(self.input[start..self.pos].iter().collect())
    }

    fn parse_cmp_op(&mut self) -> Result<CmpOp, String> {
        self.skip_whitespace();
        if self.pos + 1 < self.input.len() {
            let two: String = self.input[self.pos..self.pos + 2].iter().collect();
            match two.as_str() {
                "==" => {
                    self.pos += 2;
                    return Ok(CmpOp::Eq);
                }
                "!=" => {
                    self.pos += 2;
                    return Ok(CmpOp::Ne);
                }
                _ => {}
            }
        }
        if self.pos + 8 <= self.input.len() {
            let word: String = self.input[self.pos..self.pos + 8].iter().collect();
            if word.starts_with("contains") {
                let next = self.input.get(self.pos + 8);
                if next.is_none_or(|c| c.is_ascii_whitespace()) {
                    self.pos += 8;
                    return Ok(CmpOp::Contains);
                }
            }
        }
        Err(format!(
            "expected '==', '!=', or 'contains' at position {}",
            self.pos
        ))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.skip_whitespace();
        if self.advance() != Some('"') {
            return Err(format!("expected string at position {}", self.pos - 1));
        }
        let mut s = String::new();
        loop {
            match self.advance() {
                Some('"') => return Ok(s),
                Some('\\') => match self.advance() {
                    Some('"') => s.push('"'),
                    Some('\\') => s.push('\\'),
                    Some('n') => s.push('\n'),
                    Some(c) => {
                        s.push('\\');
                        s.push(c);
                    }
                    None => return Err("unexpected end of escape sequence".into()),
                },
                Some(c) => s.push(c),
                None => return Err("unterminated string".into()),
            }
        }
    }
}

pub fn evaluate(expr: &MatcherExpr, vars: &HashMap<String, String>) -> bool {
    match expr {
        MatcherExpr::Cmp { var, op, value } => {
            let Some(actual) = vars.get(var) else {
                return false;
            };
            match op {
                CmpOp::Eq => actual == value,
                CmpOp::Ne => actual != value,
                CmpOp::Contains => actual.contains(value.as_str()),
            }
        }
        MatcherExpr::And(l, r) => evaluate(l, vars) && evaluate(r, vars),
        MatcherExpr::Or(l, r) => evaluate(l, vars) || evaluate(r, vars),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn parse_simple_eq() {
        let expr = parse(r#"tool_name == "run_command""#).unwrap();
        assert!(evaluate(&expr, &vars(&[("tool_name", "run_command")])));
        assert!(!evaluate(&expr, &vars(&[("tool_name", "read_file")])));
    }

    #[test]
    fn parse_contains() {
        let expr = parse(r#"args.command contains "git""#).unwrap();
        assert!(evaluate(&expr, &vars(&[("args.command", "git push")])));
        assert!(!evaluate(&expr, &vars(&[("args.command", "ls -la")])));
    }

    #[test]
    fn parse_ne() {
        let expr = parse(r#"tool_name != "read_file""#).unwrap();
        assert!(evaluate(&expr, &vars(&[("tool_name", "write_file")])));
        assert!(!evaluate(&expr, &vars(&[("tool_name", "read_file")])));
    }

    #[test]
    fn parse_and() {
        let expr = parse(r#"tool_name == "run_command" && source == "user""#).unwrap();
        assert!(evaluate(
            &expr,
            &vars(&[("tool_name", "run_command"), ("source", "user")])
        ));
        assert!(!evaluate(
            &expr,
            &vars(&[("tool_name", "run_command"), ("source", "auto")])
        ));
    }

    #[test]
    fn parse_or() {
        let expr = parse(r#"tool_name == "a" || tool_name == "b""#).unwrap();
        assert!(evaluate(&expr, &vars(&[("tool_name", "a")])));
        assert!(evaluate(&expr, &vars(&[("tool_name", "b")])));
        assert!(!evaluate(&expr, &vars(&[("tool_name", "c")])));
    }

    #[test]
    fn parse_parens() {
        let expr = parse(r#"(tool_name == "a" || tool_name == "b") && source == "s""#).unwrap();
        assert!(evaluate(
            &expr,
            &vars(&[("tool_name", "a"), ("source", "s")])
        ));
        assert!(!evaluate(
            &expr,
            &vars(&[("tool_name", "a"), ("source", "x")])
        ));
    }

    #[test]
    fn unknown_var_is_false() {
        let expr = parse(r#"missing_var == "x""#).unwrap();
        assert!(!evaluate(&expr, &HashMap::new()));
    }

    #[test]
    fn parse_error_at_position() {
        let err = parse("== ").unwrap_err();
        assert!(err.contains("position"));
    }

    #[test]
    fn operator_precedence() {
        let expr = parse(r#"a == "1" || b == "2" && c == "3""#).unwrap();
        assert!(evaluate(&expr, &vars(&[("a", "1")])));
        assert!(!evaluate(&expr, &vars(&[("b", "2")])));
        assert!(evaluate(&expr, &vars(&[("b", "2"), ("c", "3")])));
    }
}
