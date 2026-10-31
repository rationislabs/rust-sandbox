use anyhow::{Context, Result, bail};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::HashSet;
use std::str::FromStr;

pub const DEFAULT_RULES: &str = include_str!("default_rules.toml");

#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    pub name: String,
    /// Monthly limit in EUR; `None` for income and unbudgeted categories.
    pub budget: Option<Decimal>,
    pub income: bool,
    /// Lowercase.
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rules {
    /// In file order: the first match wins.
    pub categories: Vec<Category>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRules {
    category: Vec<RawCategory>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCategory {
    name: String,
    budget: Option<String>,
    income: Option<bool>,
    keywords: Vec<String>,
}

pub fn parse_rules(text: &str) -> Result<Rules> {
    let raw: RawRules = toml::from_str(text).context("rules file")?;
    let mut seen = HashSet::new();
    let mut categories = Vec::with_capacity(raw.category.len());
    for c in raw.category {
        if !seen.insert(c.name.clone()) {
            bail!("category {:?} appears twice", c.name);
        }
        let budget = c
            .budget
            .map(|b| {
                Decimal::from_str(&b).with_context(|| {
                    format!("category {:?}: budget {b:?} is not a decimal", c.name)
                })
            })
            .transpose()?;
        categories.push(Category {
            keywords: c.keywords.iter().map(|k| k.to_lowercase()).collect(),
            name: c.name,
            budget,
            income: c.income.unwrap_or(false),
        });
    }
    Ok(Rules { categories })
}

/// # Panics
/// Never: a test proves the built-in rules parse.
pub fn default_rules() -> Rules {
    parse_rules(DEFAULT_RULES).expect("the built-in rules parse (tested)")
}

impl Rules {
    /// The first category with a keyword inside the payee, ignoring case.
    pub fn categorize(&self, payee: &str) -> Option<&Category> {
        let payee = payee.to_lowercase();
        self.categories
            .iter()
            .find(|c| c.keywords.iter().any(|k| payee.contains(k.as_str())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rules_parse() {
        let rules = default_rules();
        let names: Vec<&str> = rules.categories.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Income",
                "Rent",
                "Groceries",
                "Utilities",
                "Transport",
                "Eating out",
                "Subscriptions",
                "Shopping"
            ]
        );
        assert_eq!(
            rules.categories[2].budget,
            Some(Decimal::from_str("350.00").unwrap())
        );
        assert!(rules.categories[0].income && rules.categories[0].budget.is_none());
    }

    #[test]
    fn case_insensitive_substring() {
        let rules = default_rules();
        assert_eq!(
            rules.categorize("BILLA Filiale 1070").unwrap().name,
            "Groceries"
        );
        assert_eq!(rules.categorize("Café Sperl").unwrap().name, "Eating out");
        assert_eq!(rules.categorize("ÖBB Ticket").unwrap().name, "Transport");
    }

    #[test]
    fn first_match_wins() {
        let rules = parse_rules(
            "[[category]]\nname = \"A\"\nkeywords = [\"shop\"]\n[[category]]\nname = \"B\"\nkeywords = [\"shop\"]\n",
        )
        .unwrap();
        assert_eq!(rules.categorize("the shop").unwrap().name, "A");
    }

    #[test]
    fn no_match_is_none() {
        assert!(default_rules().categorize("Mystery Shop").is_none());
    }

    #[test]
    fn duplicate_name_rejected() {
        let err = parse_rules("[[category]]\nname = \"X\"\nkeywords = []\n[[category]]\nname = \"X\"\nkeywords = []\n")
            .unwrap_err();
        assert!(format!("{err:#}").contains('X'), "{err:#}");
    }

    #[test]
    fn bad_budget_rejected() {
        let err = parse_rules("[[category]]\nname = \"Y\"\nbudget = \"lots\"\nkeywords = []\n")
            .unwrap_err();
        assert!(format!("{err:#}").contains('Y'), "{err:#}");
    }
}
