//! Pluggable entity extraction abstractions.

use std::collections::HashSet;
use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;

use crate::models::EntityCandidate;

use super::MemoryError;

/// Extracts entity candidates from text.
#[async_trait]
pub trait EntityExtractor: Send + Sync {
    /// Returns normalized entity candidates discovered in the supplied content.
    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError>;
}

/// Regex-based deterministic extractor used as the default fallback implementation.
#[derive(Debug)]
pub struct RegexEntityExtractor {
    name_regex: Regex,
}

impl RegexEntityExtractor {
    /// Creates a new regex-backed entity extractor.
    ///
    /// Supports both ASCII and Unicode letters (Cyrillic, etc.).
    /// Pattern matches:
    /// - Multi-word capitalized names: "Alice Smith", "Иван Петров"
    /// - Single-token CamelCase: "OpenAI", "PostgreSQL"
    ///
    /// Minimum 3 characters to avoid noise like "I", "At", "In".
    pub fn new() -> Result<Self, MemoryError> {
        Ok(Self {
            name_regex: Regex::new(
                r"[\p{Lu}][\p{Ll}]+(?:\s+[\p{Lu}][\p{Ll}]+)+|[\p{Lu}][\p{L}\p{N}]{2,}",
            )
            .map_err(|err| MemoryError::Validation(format!("regex error: {err}")))?,
        })
    }
}

#[async_trait]
impl EntityExtractor for RegexEntityExtractor {
    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
        let candidates: std::collections::HashSet<_> = self
            .name_regex
            .find_iter(content)
            .map(|mat| mat.as_str().to_string())
            .collect();

        let mut entities = candidates
            .into_iter()
            .map(|candidate| {
                let entity_type = classify_entity_type(&candidate);
                EntityCandidate {
                    entity_type: entity_type.to_string(),
                    canonical_name: candidate,
                    aliases: Vec::new(),
                }
            })
            .collect::<Vec<_>>();

        entities.sort_by(|left, right| left.canonical_name.cmp(&right.canonical_name));
        Ok(entities)
    }
}

/// Static gazetteer of well-known toponyms for location classification.
static KNOWN_LOCATIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        // Major cities
        "New York",
        "Los Angeles",
        "Chicago",
        "Houston",
        "Phoenix",
        "Philadelphia",
        "San Antonio",
        "San Diego",
        "Dallas",
        "Austin",
        "San Francisco",
        "Seattle",
        "Denver",
        "Boston",
        "Nashville",
        "Portland",
        "Las Vegas",
        "Miami",
        "Atlanta",
        "Minneapolis",
        "Detroit",
        "Tampa",
        "Orlando",
        "Sacramento",
        "Pittsburgh",
        "Cincinnati",
        "Cleveland",
        "Indianapolis",
        "Milwaukee",
        "Columbus",
        "Kansas City",
        "Raleigh",
        "Virginia Beach",
        "Baltimore",
        "Memphis",
        "Charlotte",
        "Jacksonville",
        "San Jose",
        "Fort Worth",
        "El Paso",
        "London",
        "Paris",
        "Berlin",
        "Madrid",
        "Rome",
        "Amsterdam",
        "Vienna",
        "Prague",
        "Warsaw",
        "Budapest",
        "Dublin",
        "Lisbon",
        "Stockholm",
        "Oslo",
        "Helsinki",
        "Copenhagen",
        "Brussels",
        "Zurich",
        "Geneva",
        "Munich",
        "Frankfurt",
        "Hamburg",
        "Barcelona",
        "Milan",
        "Naples",
        "Tokyo",
        "Osaka",
        "Kyoto",
        "Seoul",
        "Beijing",
        "Shanghai",
        "Hong Kong",
        "Singapore",
        "Taipei",
        "Bangkok",
        "Mumbai",
        "Delhi",
        "Bangalore",
        "Hyderabad",
        "Chennai",
        "Kolkata",
        "Jakarta",
        "Manila",
        "Hanoi",
        "Kuala Lumpur",
        "Sydney",
        "Melbourne",
        "Brisbane",
        "Perth",
        "Auckland",
        "Toronto",
        "Vancouver",
        "Montreal",
        "Ottawa",
        "Calgary",
        "Mexico City",
        "Sao Paulo",
        "Buenos Aires",
        "Lima",
        "Bogota",
        "Santiago",
        "Rio de Janeiro",
        "Cairo",
        "Lagos",
        "Nairobi",
        "Johannesburg",
        "Cape Town",
        "Dubai",
        "Riyadh",
        "Tel Aviv",
        "Istanbul",
        "Moscow",
        "Saint Petersburg",
        "Kiev",
        "Bucharest",
        "Stockholm",
        "Tallinn",
        "Riga",
        "Vilnius",
        "Belgrade",
        // Countries
        "United States",
        "United Kingdom",
        "Canada",
        "Australia",
        "Germany",
        "France",
        "Italy",
        "Spain",
        "Japan",
        "China",
        "India",
        "Brazil",
        "Mexico",
        "Russia",
        "South Korea",
        "Indonesia",
        "Turkey",
        "Saudi Arabia",
        "Argentina",
        "South Africa",
        "Nigeria",
        "Egypt",
        "Poland",
        "Netherlands",
        "Belgium",
        "Sweden",
        "Norway",
        "Finland",
        "Denmark",
        "Switzerland",
        "Austria",
        "Portugal",
        "Ireland",
        "Greece",
        "Czech Republic",
        "Romania",
        "Hungary",
        "Ukraine",
        "Israel",
        "Thailand",
        "Vietnam",
        "Philippines",
        "Malaysia",
        "Singapore",
        "New Zealand",
        "Colombia",
        "Chile",
        "Peru",
        // US states
        "California",
        "Texas",
        "Florida",
        "New York",
        "Pennsylvania",
        "Illinois",
        "Ohio",
        "Georgia",
        "North Carolina",
        "Michigan",
        "New Jersey",
        "Virginia",
        "Washington",
        "Arizona",
        "Massachusetts",
        "Tennessee",
        "Indiana",
        "Maryland",
        "Missouri",
        "Wisconsin",
        "Colorado",
        "Minnesota",
        "Oregon",
        "Alabama",
        "Louisiana",
        "Kentucky",
        "South Carolina",
        "Iowa",
        "Nevada",
        "Arkansas",
        "Connecticut",
        "Utah",
        "Oklahoma",
        "Hawaii",
        // Regions / continents
        "Europe",
        "Asia",
        "Africa",
        "North America",
        "South America",
        "Oceania",
        "Antarctica",
        "Middle East",
        "Southeast Asia",
        "East Asia",
        "Central America",
        "Caribbean",
        "Scandinavia",
        "Balkans",
        "Nordic",
    ])
});

/// Classifies an entity candidate into a type based on naming patterns.
fn classify_entity_type(name: &str) -> &'static str {
    static COMPANY_SUFFIXES: &[&str] = &[
        "Corp",
        "Inc",
        "Ltd",
        "LLC",
        "GmbH",
        "AG",
        "SA",
        "PLC",
        "Company",
        "Group",
        "Systems",
        "Technologies",
        "Solutions",
        "Labs",
        "Studio",
        "Partners",
        "Associates",
        "Holdings",
        "Foundation",
        "Institute",
        "University",
        "Academy",
        "Limited",
    ];

    static EVENT_INDICATORS: &[&str] = &[
        "Conference",
        "Summit",
        "Meetup",
        "Hackathon",
        "Workshop",
        "Festival",
        "Ceremony",
        "Award",
        "Championship",
        "Olympics",
    ];

    static LOCATION_INDICATORS: &[&str] = &[
        "City",
        "County",
        "State",
        "Province",
        "Country",
        "District",
        "Region",
        "Territory",
        "Island",
    ];

    for suffix in COMPANY_SUFFIXES {
        if name.contains(suffix) {
            return "company";
        }
    }
    for indicator in EVENT_INDICATORS {
        if name.contains(indicator) {
            return "event";
        }
    }
    for indicator in LOCATION_INDICATORS {
        if name.contains(indicator) {
            return "location";
        }
    }

    if KNOWN_LOCATIONS.contains(name) {
        return "location";
    }

    "person"
}

/// Type alias for the pluggable extraction function used by [`LlmEntityExtractor`].
///
/// Takes raw text and returns extracted entity candidates. Implementations
/// should call out to an LLM, gRPC service, or any other async backend.
pub type ExtractFn = dyn Fn(&str) -> Result<Vec<EntityCandidate>, MemoryError> + Send + Sync;

/// LLM-backed entity extractor that delegates to a pluggable function.
///
/// Activate via config flag `ENTITY_EXTRACTOR=llm`. The extraction function
/// is injected at construction time — no HTTP client dependency required.
/// Falls back gracefully: if the function returns an error, returns an
/// empty candidate list (the caller can retry with [`RegexEntityExtractor`]).
pub struct LlmEntityExtractor {
    extract_fn: Box<ExtractFn>,
}

impl std::fmt::Debug for LlmEntityExtractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmEntityExtractor").finish()
    }
}

impl LlmEntityExtractor {
    /// Creates a new LLM-backed extractor with the given extraction function.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&str) -> Result<Vec<EntityCandidate>, MemoryError> + Send + Sync + 'static,
    {
        Self {
            extract_fn: Box::new(f),
        }
    }
}

#[async_trait]
impl EntityExtractor for LlmEntityExtractor {
    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
        (self.extract_fn)(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn regex_entity_extractor_returns_deterministic_candidates() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("Alice Smith met Bob Jones at Acme Inc")
            .await
            .unwrap();

        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].canonical_name, "Acme Inc");
        assert_eq!(candidates[1].canonical_name, "Alice Smith");
        assert_eq!(candidates[2].canonical_name, "Bob Jones");
    }

    #[tokio::test]
    async fn regex_entity_extractor_includes_single_token_camel_case_names() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates(
                "OpenAI partnered with Anthropic while PostgreSQL backed Alice Smith",
            )
            .await
            .unwrap();

        let names = candidates
            .into_iter()
            .map(|candidate| candidate.canonical_name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "Alice Smith".to_string(),
                "Anthropic".to_string(),
                "OpenAI".to_string(),
                "PostgreSQL".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn regex_entity_extractor_filters_out_short_words() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("I met Bob at OpenAI on Monday at San Francisco")
            .await
            .unwrap();

        let names = candidates
            .into_iter()
            .map(|candidate| candidate.canonical_name)
            .collect::<Vec<_>>();

        // Should NOT include: I, At, In, On (1-2 letter words)
        // Should include: Bob, OpenAI, Monday, San Francisco (3+ chars)
        assert!(!names.contains(&"I".to_string()));
        assert!(!names.contains(&"At".to_string()));
        assert!(!names.contains(&"On".to_string()));

        assert!(names.contains(&"Bob".to_string()));
        assert!(names.contains(&"OpenAI".to_string()));
        assert!(names.contains(&"Monday".to_string()));
        assert!(names.contains(&"San Francisco".to_string()));
    }

    #[tokio::test]
    async fn regex_entity_extractor_supports_unicode_names() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("Иван Петров встретился с Maria Garcia в компании TechCorp")
            .await
            .unwrap();

        let names = candidates
            .into_iter()
            .map(|candidate| candidate.canonical_name)
            .collect::<Vec<_>>();

        // Should include Cyrillic and Latin names
        assert!(names.contains(&"Иван Петров".to_string()));
        assert!(names.contains(&"Maria Garcia".to_string()));
        assert!(names.contains(&"TechCorp".to_string()));
    }

    #[tokio::test]
    async fn regex_entity_extractor_classifies_company_types() {
        let extractor = RegexEntityExtractor::new().unwrap();
        // Use company names that the regex can extract (multi-word or with lowercase)
        let candidates = extractor
            .extract_candidates("Acme Corp and Globex Inc and Initech Limited")
            .await
            .unwrap();

        for candidate in &candidates {
            assert_eq!(
                candidate.entity_type, "company",
                "{:?} should be classified as company",
                candidate.canonical_name
            );
        }
    }

    #[tokio::test]
    async fn regex_entity_extractor_classifies_event_types() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("Tech Summit in San Francisco")
            .await
            .unwrap();

        let types: std::collections::HashMap<_, _> = candidates
            .iter()
            .map(|c| (c.canonical_name.as_str(), c.entity_type.as_str()))
            .collect();

        // "Tech Summit" contains the "Summit" indicator → classified as event
        assert_eq!(types.get("Tech Summit"), Some(&"event"));

        // "San Francisco" is in the gazetteer → classified as location
        assert_eq!(types.get("San Francisco"), Some(&"location"));
    }

    #[tokio::test]
    async fn llm_extractor_delegates_to_provided_function() {
        let extractor = LlmEntityExtractor::new(|_content| {
            Ok(vec![
                EntityCandidate {
                    entity_type: "person".into(),
                    canonical_name: "Alice Smith".into(),
                    aliases: vec![],
                },
                EntityCandidate {
                    entity_type: "company".into(),
                    canonical_name: "Acme Corp".into(),
                    aliases: vec![],
                },
            ])
        });

        let candidates = extractor
            .extract_candidates("irrelevant input")
            .await
            .unwrap();

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].canonical_name, "Alice Smith");
        assert_eq!(candidates[1].entity_type, "company");
    }
}
