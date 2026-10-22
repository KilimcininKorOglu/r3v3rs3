//! Country metadata, regional presets, and helper functions for GeoIP access control.

/// Basic information about an ISO 3166-1 alpha-2 country.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CountryInfo {
    pub code: &'static str,
    pub name_en: &'static str,
    pub name_tr: &'static str,
}

/// A pre-configured regional group of countries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionalPreset {
    pub id: &'static str,
    pub name_en: &'static str,
    pub name_tr: &'static str,
    pub countries: &'static [&'static str],
}

/// List of standard countries with English and Turkish localized names.
pub static ALL_COUNTRIES: &[CountryInfo] = &[
    CountryInfo {
        code: "AD",
        name_en: "Andorra",
        name_tr: "Andorra",
    },
    CountryInfo {
        code: "AE",
        name_en: "United Arab Emirates",
        name_tr: "Birleşik Arap Emirlikleri",
    },
    CountryInfo {
        code: "AF",
        name_en: "Afghanistan",
        name_tr: "Afganistan",
    },
    CountryInfo {
        code: "AG",
        name_en: "Antigua and Barbuda",
        name_tr: "Antigua ve Barbuda",
    },
    CountryInfo {
        code: "AL",
        name_en: "Albania",
        name_tr: "Arnavutluk",
    },
    CountryInfo {
        code: "AM",
        name_en: "Armenia",
        name_tr: "Ermenistan",
    },
    CountryInfo {
        code: "AO",
        name_en: "Angola",
        name_tr: "Angola",
    },
    CountryInfo {
        code: "AR",
        name_en: "Argentina",
        name_tr: "Arjantin",
    },
    CountryInfo {
        code: "AT",
        name_en: "Austria",
        name_tr: "Avusturya",
    },
    CountryInfo {
        code: "AU",
        name_en: "Australia",
        name_tr: "Avustralya",
    },
    CountryInfo {
        code: "AZ",
        name_en: "Azerbaijan",
        name_tr: "Azerbaycan",
    },
    CountryInfo {
        code: "BA",
        name_en: "Bosnia and Herzegovina",
        name_tr: "Bosna Hersek",
    },
    CountryInfo {
        code: "BB",
        name_en: "Barbados",
        name_tr: "Barbados",
    },
    CountryInfo {
        code: "BD",
        name_en: "Bangladesh",
        name_tr: "Bangladeş",
    },
    CountryInfo {
        code: "BE",
        name_en: "Belgium",
        name_tr: "Belçika",
    },
    CountryInfo {
        code: "BF",
        name_en: "Burkina Faso",
        name_tr: "Burkina Faso",
    },
    CountryInfo {
        code: "BG",
        name_en: "Bulgaria",
        name_tr: "Bulgaristan",
    },
    CountryInfo {
        code: "BH",
        name_en: "Bahrain",
        name_tr: "Bahreyn",
    },
    CountryInfo {
        code: "BI",
        name_en: "Burundi",
        name_tr: "Burundi",
    },
    CountryInfo {
        code: "BJ",
        name_en: "Benin",
        name_tr: "Benin",
    },
    CountryInfo {
        code: "BN",
        name_en: "Brunei",
        name_tr: "Brunei",
    },
    CountryInfo {
        code: "BO",
        name_en: "Bolivia",
        name_tr: "Bolivya",
    },
    CountryInfo {
        code: "BR",
        name_en: "Brazil",
        name_tr: "Brezilya",
    },
    CountryInfo {
        code: "BS",
        name_en: "Bahamas",
        name_tr: "Bahamalar",
    },
    CountryInfo {
        code: "BT",
        name_en: "Bhutan",
        name_tr: "Butan",
    },
    CountryInfo {
        code: "BW",
        name_en: "Botswana",
        name_tr: "Botsvana",
    },
    CountryInfo {
        code: "BY",
        name_en: "Belarus",
        name_tr: "Belarus",
    },
    CountryInfo {
        code: "BZ",
        name_en: "Belize",
        name_tr: "Belize",
    },
    CountryInfo {
        code: "CA",
        name_en: "Canada",
        name_tr: "Kanada",
    },
    CountryInfo {
        code: "CD",
        name_en: "DR Congo",
        name_tr: "Kongo DC",
    },
    CountryInfo {
        code: "CF",
        name_en: "Central African Republic",
        name_tr: "Orta Afrika Cumhuriyeti",
    },
    CountryInfo {
        code: "CG",
        name_en: "Republic of the Congo",
        name_tr: "Kongo Cumhuriyeti",
    },
    CountryInfo {
        code: "CH",
        name_en: "Switzerland",
        name_tr: "İsviçre",
    },
    CountryInfo {
        code: "CI",
        name_en: "Ivory Coast",
        name_tr: "Fildişi Sahili",
    },
    CountryInfo {
        code: "CL",
        name_en: "Chile",
        name_tr: "Şili",
    },
    CountryInfo {
        code: "CM",
        name_en: "Cameroon",
        name_tr: "Kamerun",
    },
    CountryInfo {
        code: "CN",
        name_en: "China",
        name_tr: "Çin",
    },
    CountryInfo {
        code: "CO",
        name_en: "Colombia",
        name_tr: "Kolombiya",
    },
    CountryInfo {
        code: "CR",
        name_en: "Costa Rica",
        name_tr: "Kosta Rika",
    },
    CountryInfo {
        code: "CU",
        name_en: "Cuba",
        name_tr: "Küba",
    },
    CountryInfo {
        code: "CV",
        name_en: "Cape Verde",
        name_tr: "Yeşil Burun Adaları",
    },
    CountryInfo {
        code: "CY",
        name_en: "Cyprus",
        name_tr: "Kıbrıs",
    },
    CountryInfo {
        code: "CZ",
        name_en: "Czech Republic",
        name_tr: "Çek Cumhuriyeti",
    },
    CountryInfo {
        code: "DE",
        name_en: "Germany",
        name_tr: "Almanya",
    },
    CountryInfo {
        code: "DJ",
        name_en: "Djibouti",
        name_tr: "Cibuti",
    },
    CountryInfo {
        code: "DK",
        name_en: "Denmark",
        name_tr: "Danimarka",
    },
    CountryInfo {
        code: "DM",
        name_en: "Dominica",
        name_tr: "Dominika",
    },
    CountryInfo {
        code: "DO",
        name_en: "Dominican Republic",
        name_tr: "Dominik Cumhuriyeti",
    },
    CountryInfo {
        code: "DZ",
        name_en: "Algeria",
        name_tr: "Cezayir",
    },
    CountryInfo {
        code: "EC",
        name_en: "Ecuador",
        name_tr: "Ekvador",
    },
    CountryInfo {
        code: "EE",
        name_en: "Estonia",
        name_tr: "Estonya",
    },
    CountryInfo {
        code: "EG",
        name_en: "Egypt",
        name_tr: "Mısır",
    },
    CountryInfo {
        code: "ES",
        name_en: "Spain",
        name_tr: "İspanya",
    },
    CountryInfo {
        code: "ET",
        name_en: "Ethiopia",
        name_tr: "Etiyopya",
    },
    CountryInfo {
        code: "FI",
        name_en: "Finland",
        name_tr: "Finlandiya",
    },
    CountryInfo {
        code: "FJ",
        name_en: "Fiji",
        name_tr: "Fiji",
    },
    CountryInfo {
        code: "FR",
        name_en: "France",
        name_tr: "Fransa",
    },
    CountryInfo {
        code: "GA",
        name_en: "Gabon",
        name_tr: "Gabon",
    },
    CountryInfo {
        code: "GB",
        name_en: "United Kingdom",
        name_tr: "Birleşik Krallık",
    },
    CountryInfo {
        code: "GE",
        name_en: "Georgia",
        name_tr: "Gürcistan",
    },
    CountryInfo {
        code: "GH",
        name_en: "Ghana",
        name_tr: "Gana",
    },
    CountryInfo {
        code: "GM",
        name_en: "Gambia",
        name_tr: "Gambiya",
    },
    CountryInfo {
        code: "GN",
        name_en: "Guinea",
        name_tr: "Gine",
    },
    CountryInfo {
        code: "GR",
        name_en: "Greece",
        name_tr: "Yunanistan",
    },
    CountryInfo {
        code: "GT",
        name_en: "Guatemala",
        name_tr: "Guatemala",
    },
    CountryInfo {
        code: "HK",
        name_en: "Hong Kong",
        name_tr: "Hong Kong",
    },
    CountryInfo {
        code: "HN",
        name_en: "Honduras",
        name_tr: "Honduras",
    },
    CountryInfo {
        code: "HR",
        name_en: "Croatia",
        name_tr: "Hırvatistan",
    },
    CountryInfo {
        code: "HT",
        name_en: "Haiti",
        name_tr: "Haiti",
    },
    CountryInfo {
        code: "HU",
        name_en: "Hungary",
        name_tr: "Macaristan",
    },
    CountryInfo {
        code: "ID",
        name_en: "Indonesia",
        name_tr: "Endonezya",
    },
    CountryInfo {
        code: "IE",
        name_en: "Ireland",
        name_tr: "İrlanda",
    },
    CountryInfo {
        code: "IL",
        name_en: "Israel",
        name_tr: "İsrail",
    },
    CountryInfo {
        code: "IN",
        name_en: "India",
        name_tr: "Hindistan",
    },
    CountryInfo {
        code: "IQ",
        name_en: "Iraq",
        name_tr: "Irak",
    },
    CountryInfo {
        code: "IR",
        name_en: "Iran",
        name_tr: "İran",
    },
    CountryInfo {
        code: "IS",
        name_en: "Iceland",
        name_tr: "İzlanda",
    },
    CountryInfo {
        code: "IT",
        name_en: "Italy",
        name_tr: "İtalya",
    },
    CountryInfo {
        code: "JM",
        name_en: "Jamaica",
        name_tr: "Jamaika",
    },
    CountryInfo {
        code: "JO",
        name_en: "Jordan",
        name_tr: "Ürdün",
    },
    CountryInfo {
        code: "JP",
        name_en: "Japan",
        name_tr: "Japonya",
    },
    CountryInfo {
        code: "KE",
        name_en: "Kenya",
        name_tr: "Kenya",
    },
    CountryInfo {
        code: "KG",
        name_en: "Kyrgyzstan",
        name_tr: "Kırgızistan",
    },
    CountryInfo {
        code: "KH",
        name_en: "Cambodia",
        name_tr: "Kamboçya",
    },
    CountryInfo {
        code: "KR",
        name_en: "South Korea",
        name_tr: "Güney Kore",
    },
    CountryInfo {
        code: "KW",
        name_en: "Kuwait",
        name_tr: "Kuveyt",
    },
    CountryInfo {
        code: "KZ",
        name_en: "Kazakhstan",
        name_tr: "Kazakistan",
    },
    CountryInfo {
        code: "LA",
        name_en: "Laos",
        name_tr: "Laos",
    },
    CountryInfo {
        code: "LB",
        name_en: "Lebanon",
        name_tr: "Lübnan",
    },
    CountryInfo {
        code: "LI",
        name_en: "Liechtenstein",
        name_tr: "Lihtenştayn",
    },
    CountryInfo {
        code: "LK",
        name_en: "Sri Lanka",
        name_tr: "Sri Lanka",
    },
    CountryInfo {
        code: "LT",
        name_en: "Lithuania",
        name_tr: "Litvanya",
    },
    CountryInfo {
        code: "LU",
        name_en: "Luxembourg",
        name_tr: "Lüksemburg",
    },
    CountryInfo {
        code: "LV",
        name_en: "Latvia",
        name_tr: "Letonya",
    },
    CountryInfo {
        code: "LY",
        name_en: "Libya",
        name_tr: "Libya",
    },
    CountryInfo {
        code: "MA",
        name_en: "Morocco",
        name_tr: "Fas",
    },
    CountryInfo {
        code: "MC",
        name_en: "Monaco",
        name_tr: "Monako",
    },
    CountryInfo {
        code: "MD",
        name_en: "Moldova",
        name_tr: "Moldova",
    },
    CountryInfo {
        code: "ME",
        name_en: "Montenegro",
        name_tr: "Karadağ",
    },
    CountryInfo {
        code: "MG",
        name_en: "Madagascar",
        name_tr: "Madagaskar",
    },
    CountryInfo {
        code: "MK",
        name_en: "North Macedonia",
        name_tr: "Kuzey Makedonya",
    },
    CountryInfo {
        code: "ML",
        name_en: "Mali",
        name_tr: "Mali",
    },
    CountryInfo {
        code: "MN",
        name_en: "Mongolia",
        name_tr: "Moğolistan",
    },
    CountryInfo {
        code: "MT",
        name_en: "Malta",
        name_tr: "Malta",
    },
    CountryInfo {
        code: "MU",
        name_en: "Mauritius",
        name_tr: "Mauritius",
    },
    CountryInfo {
        code: "MV",
        name_en: "Maldives",
        name_tr: "Maldivler",
    },
    CountryInfo {
        code: "MX",
        name_en: "Mexico",
        name_tr: "Meksika",
    },
    CountryInfo {
        code: "MY",
        name_en: "Malaysia",
        name_tr: "Malezya",
    },
    CountryInfo {
        code: "MZ",
        name_en: "Mozambique",
        name_tr: "Mozambik",
    },
    CountryInfo {
        code: "NA",
        name_en: "Namibia",
        name_tr: "Namibya",
    },
    CountryInfo {
        code: "NE",
        name_en: "Niger",
        name_tr: "Nijer",
    },
    CountryInfo {
        code: "NG",
        name_en: "Nigeria",
        name_tr: "Nijerya",
    },
    CountryInfo {
        code: "NL",
        name_en: "Netherlands",
        name_tr: "Hollanda",
    },
    CountryInfo {
        code: "NO",
        name_en: "Norway",
        name_tr: "Norveç",
    },
    CountryInfo {
        code: "NP",
        name_en: "Nepal",
        name_tr: "Nepal",
    },
    CountryInfo {
        code: "NZ",
        name_en: "New Zealand",
        name_tr: "Yeni Zelanda",
    },
    CountryInfo {
        code: "OM",
        name_en: "Oman",
        name_tr: "Umman",
    },
    CountryInfo {
        code: "PA",
        name_en: "Panama",
        name_tr: "Panama",
    },
    CountryInfo {
        code: "PE",
        name_en: "Peru",
        name_tr: "Peru",
    },
    CountryInfo {
        code: "PH",
        name_en: "Philippines",
        name_tr: "Filipinler",
    },
    CountryInfo {
        code: "PK",
        name_en: "Pakistan",
        name_tr: "Pakistan",
    },
    CountryInfo {
        code: "PL",
        name_en: "Poland",
        name_tr: "Polonya",
    },
    CountryInfo {
        code: "PT",
        name_en: "Portugal",
        name_tr: "Portekiz",
    },
    CountryInfo {
        code: "PY",
        name_en: "Paraguay",
        name_tr: "Paraguay",
    },
    CountryInfo {
        code: "QA",
        name_en: "Qatar",
        name_tr: "Katar",
    },
    CountryInfo {
        code: "RO",
        name_en: "Romania",
        name_tr: "Romanya",
    },
    CountryInfo {
        code: "RS",
        name_en: "Serbia",
        name_tr: "Sırbistan",
    },
    CountryInfo {
        code: "RU",
        name_en: "Russia",
        name_tr: "Rusya",
    },
    CountryInfo {
        code: "RW",
        name_en: "Rwanda",
        name_tr: "Ruanda",
    },
    CountryInfo {
        code: "SA",
        name_en: "Saudi Arabia",
        name_tr: "Suudi Arabistan",
    },
    CountryInfo {
        code: "SE",
        name_en: "Sweden",
        name_tr: "İsveç",
    },
    CountryInfo {
        code: "SG",
        name_en: "Singapore",
        name_tr: "Singapur",
    },
    CountryInfo {
        code: "SI",
        name_en: "Slovenia",
        name_tr: "Slovenya",
    },
    CountryInfo {
        code: "SK",
        name_en: "Slovakia",
        name_tr: "Slovakya",
    },
    CountryInfo {
        code: "SN",
        name_en: "Senegal",
        name_tr: "Senegal",
    },
    CountryInfo {
        code: "SO",
        name_en: "Somalia",
        name_tr: "Somali",
    },
    CountryInfo {
        code: "SY",
        name_en: "Syria",
        name_tr: "Suriye",
    },
    CountryInfo {
        code: "TH",
        name_en: "Thailand",
        name_tr: "Tayland",
    },
    CountryInfo {
        code: "TJ",
        name_en: "Tajikistan",
        name_tr: "Tacikistan",
    },
    CountryInfo {
        code: "TM",
        name_en: "Turkmenistan",
        name_tr: "Türkmenistan",
    },
    CountryInfo {
        code: "TN",
        name_en: "Tunisia",
        name_tr: "Tunus",
    },
    CountryInfo {
        code: "TR",
        name_en: "Turkey",
        name_tr: "Türkiye",
    },
    CountryInfo {
        code: "TW",
        name_en: "Taiwan",
        name_tr: "Tayvan",
    },
    CountryInfo {
        code: "TZ",
        name_en: "Tanzania",
        name_tr: "Tanzanya",
    },
    CountryInfo {
        code: "UA",
        name_en: "Ukraine",
        name_tr: "Ukrayna",
    },
    CountryInfo {
        code: "UG",
        name_en: "Uganda",
        name_tr: "Uganda",
    },
    CountryInfo {
        code: "US",
        name_en: "United States",
        name_tr: "Amerika Birleşik Devletleri",
    },
    CountryInfo {
        code: "UY",
        name_en: "Uruguay",
        name_tr: "Uruguay",
    },
    CountryInfo {
        code: "UZ",
        name_en: "Uzbekistan",
        name_tr: "Özbekistan",
    },
    CountryInfo {
        code: "VE",
        name_en: "Venezuela",
        name_tr: "Venezuela",
    },
    CountryInfo {
        code: "VN",
        name_en: "Vietnam",
        name_tr: "Vietnam",
    },
    CountryInfo {
        code: "YE",
        name_en: "Yemen",
        name_tr: "Yemen",
    },
    CountryInfo {
        code: "ZA",
        name_en: "South Africa",
        name_tr: "Güney Afrika",
    },
    CountryInfo {
        code: "ZM",
        name_en: "Zambia",
        name_tr: "Zambiya",
    },
    CountryInfo {
        code: "ZW",
        name_en: "Zimbabwe",
        name_tr: "Zimbabve",
    },
];

/// Pre-configured regional presets for single-click policy generation.
pub static REGIONAL_PRESETS: &[RegionalPreset] = &[
    RegionalPreset {
        id: "turkey",
        name_en: "Turkey Only (TR)",
        name_tr: "Yalnızca Türkiye (TR)",
        countries: &["TR"],
    },
    RegionalPreset {
        id: "eu",
        name_en: "European Union (EU 27)",
        name_tr: "Avrupa Birliği (AB 27)",
        countries: &[
            "AT", "BE", "BG", "CY", "CZ", "DE", "DK", "EE", "ES", "FI", "FR", "GR", "HR", "HU",
            "IE", "IT", "LT", "LU", "LV", "MT", "NL", "PL", "PT", "RO", "SE", "SI", "SK",
        ],
    },
    RegionalPreset {
        id: "na",
        name_en: "North America (US, CA, MX)",
        name_tr: "Kuzey Amerika (ABD, CA, MX)",
        countries: &["US", "CA", "MX"],
    },
    RegionalPreset {
        id: "g7",
        name_en: "G7 Nations",
        name_tr: "G7 Ülkeleri",
        countries: &["US", "CA", "GB", "DE", "FR", "IT", "JP"],
    },
    RegionalPreset {
        id: "dach",
        name_en: "DACH (DE, AT, CH)",
        name_tr: "DACH (DE, AT, CH)",
        countries: &["DE", "AT", "CH"],
    },
    RegionalPreset {
        id: "apac",
        name_en: "Asia-Pacific Core (APAC)",
        name_tr: "Asya-Pasifik (APAC)",
        countries: &["JP", "KR", "SG", "AU", "NZ", "IN"],
    },
    RegionalPreset {
        id: "latam",
        name_en: "Latin America (LATAM)",
        name_tr: "Latin Amerika (LATAM)",
        countries: &["BR", "AR", "CL", "CO", "PE", "MX"],
    },
    RegionalPreset {
        id: "mena",
        name_en: "Middle East & North Africa (MENA)",
        name_tr: "Orta Doğu & Kuzey Afrika (MENA)",
        countries: &["TR", "SA", "AE", "EG", "QA", "KW", "OM", "BH", "JO"],
    },
    RegionalPreset {
        id: "nordic",
        name_en: "Nordic Countries",
        name_tr: "İskandinav Ülkeleri",
        countries: &["SE", "NO", "FI", "DK", "IS"],
    },
];

/// Looks up country information by its ISO 3166-1 alpha-2 code (case-insensitive).
pub fn lookup_country(code: &str) -> Option<&'static CountryInfo> {
    let needle = code.trim();
    ALL_COUNTRIES
        .iter()
        .find(|c| c.code.eq_ignore_ascii_case(needle))
}

/// Returns a localized display name for a country code, falling back to the code itself.
pub fn country_name(code: &str, is_tr: bool) -> &'static str {
    if let Some(info) = lookup_country(code) {
        if is_tr { info.name_tr } else { info.name_en }
    } else {
        ""
    }
}

/// Looks up a preset by identifier.
pub fn lookup_preset(id: &str) -> Option<&'static RegionalPreset> {
    REGIONAL_PRESETS.iter().find(|p| p.id == id)
}

/// Parses a comma- or whitespace-separated list of country codes into clean, uppercase codes.
pub fn parse_countries(input: &str) -> Vec<String> {
    let mut result = Vec::new();
    for part in input.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let code = part.trim().to_uppercase();
        if code.len() == 2 && !result.contains(&code) {
            result.push(code);
        }
    }
    result
}

/// Formats a list of country codes into a comma-separated string.
pub fn format_countries(codes: &[String]) -> String {
    codes.join(", ")
}

/// Adds a country code to a list if not already present.
pub fn add_country(codes: &[String], code: &str) -> Vec<String> {
    let clean = code.trim().to_uppercase();
    let mut updated = codes.to_vec();
    if clean.len() == 2 && !updated.contains(&clean) {
        updated.push(clean);
    }
    updated
}

/// Removes a country code from a list.
pub fn remove_country(codes: &[String], code: &str) -> Vec<String> {
    let clean = code.trim().to_uppercase();
    codes.iter().filter(|c| *c != &clean).cloned().collect()
}

/// Returns the country codes for a given preset ID.
pub fn apply_preset(preset_id: &str) -> Option<Vec<String>> {
    lookup_preset(preset_id).map(|p| p.countries.iter().map(|&c| c.to_string()).collect())
}

/// Searches countries matching a query string across code, English name and Turkish name.
pub fn search_countries(query: &str, is_tr: bool, limit: usize) -> Vec<&'static CountryInfo> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return ALL_COUNTRIES.iter().take(limit).collect();
    }
    let mut matches = Vec::new();
    for country in ALL_COUNTRIES {
        if matches_query(country, &q, is_tr) {
            matches.push(country);
            if matches.len() >= limit {
                break;
            }
        }
    }
    matches
}

fn matches_query(country: &CountryInfo, needle: &str, is_tr: bool) -> bool {
    country.code.to_lowercase().contains(needle)
        || (is_tr && country.name_tr.to_lowercase().contains(needle))
        || (!is_tr && country.name_en.to_lowercase().contains(needle))
        || country.name_en.to_lowercase().contains(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_country() {
        assert_eq!(lookup_country("TR").map(|c| c.name_en), Some("Turkey"));
        assert_eq!(lookup_country("tr").map(|c| c.name_tr), Some("Türkiye"));
        assert_eq!(
            lookup_country("US").map(|c| c.name_en),
            Some("United States")
        );
        assert_eq!(lookup_country("XX"), None);
    }

    #[test]
    fn test_parse_and_format_countries() {
        let input = "TR, us; de   fr,tr";
        let parsed = parse_countries(input);
        assert_eq!(parsed, vec!["TR", "US", "DE", "FR"]);
        assert_eq!(format_countries(&parsed), "TR, US, DE, FR");
    }

    #[test]
    fn test_add_and_remove_country() {
        let base = vec!["TR".to_string(), "US".to_string()];
        let added = add_country(&base, "de");
        assert_eq!(added, vec!["TR", "US", "DE"]);
        let dup = add_country(&added, "TR");
        assert_eq!(dup, vec!["TR", "US", "DE"]);
        let removed = remove_country(&added, "US");
        assert_eq!(removed, vec!["TR", "DE"]);
    }

    #[test]
    fn test_apply_preset() {
        let tr = apply_preset("turkey").unwrap();
        assert_eq!(tr, vec!["TR"]);

        let eu = apply_preset("eu").unwrap();
        assert_eq!(eu.len(), 27);
        assert!(eu.contains(&"DE".to_string()));
        assert!(eu.contains(&"FR".to_string()));

        let na = apply_preset("na").unwrap();
        assert_eq!(na, vec!["US", "CA", "MX"]);

        assert_eq!(apply_preset("nonexistent"), None);
    }

    #[test]
    fn test_search_countries() {
        let res_en = search_countries("germany", false, 5);
        assert_eq!(res_en.len(), 1);
        assert_eq!(res_en[0].code, "DE");

        let res_tr = search_countries("almanya", true, 5);
        assert_eq!(res_tr.len(), 1);
        assert_eq!(res_tr[0].code, "DE");

        let res_code = search_countries("tr", false, 5);
        assert!(res_code.iter().any(|c| c.code == "TR"));
    }
}
