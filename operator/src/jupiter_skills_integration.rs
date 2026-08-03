//! Jupiter Skills Integration
//!
//! Integration layer for leveraging newly installed Jupiter skills:
//! - jupiter-swap-migration: Migration from v1 to v2
//! - integrating-jupiter: Best practices and integration patterns
//! - jupiter-lend: Future lending protocol integration
//! - jupiter-vrfd: Token verification integration

use crate::config::JupiterConfig;
use crate::error::AppResult;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Jupiter API version information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JupiterApiVersion {
    /// API version (e.g., "v1", "v2")
    pub version: String,
    /// Base URL
    pub base_url: String,
    /// Available endpoints
    pub endpoints: Vec<String>,
    /// Supported features
    pub features: Vec<String>,
    /// Migration status
    pub migration_required: bool,
    /// Deprecation deadline (if applicable)
    pub deprecation_deadline: Option<String>,
}

/// Jupiter skill integration status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JupiterSkillsStatus {
    /// Available skills
    pub available_skills: Vec<String>,
    /// Integration recommendations
    pub recommendations: Vec<String>,
    /// Migration status
    pub migration_status: MigrationStatus,
    /// Best practice compliance
    pub best_practice_compliance: HashMap<String, bool>,
}

/// Migration status for Jupiter APIs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationStatus {
    /// Swap API migration status
    pub swap_api: SwapApiStatus,
    /// Price API migration status
    pub price_api: PriceApiStatus,
    /// Lite API migration status
    pub lite_api: LiteApiStatus,
    /// Required migrations
    pub required_migrations: Vec<String>,
    /// Optional migrations
    pub optional_migrations: Vec<String>,
}

/// Swap API version status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SwapApiStatus {
    V1,
    V2,
    Migrated,
}

/// Price API version status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PriceApiStatus {
    V2,
    V3,
    Migrated,
}

/// Lite API migration status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LiteApiStatus {
    Pending,
    Migrated,
}

/// Jupiter skills integration utilities
pub struct JupiterSkillsIntegration {
    /// Current configuration
    config: JupiterConfig,
}

impl JupiterSkillsIntegration {
    /// Create new Jupiter skills integration
    pub fn new(config: JupiterConfig) -> Self {
        Self { config }
    }

    /// Get Jupiter API version information
    pub fn get_api_version_info(&self) -> JupiterApiVersion {
        let version = if self.config.api_url.contains("/v2") {
            "v2"
        } else if self.config.api_url.contains("/v1") {
            "v1"
        } else {
            "unknown"
        };

        let base_url = self.base_url();

        let endpoints = if version == "v2" {
            vec![
                "/order".to_string(),
                "/build".to_string(),
                "/execute".to_string(),
                "/quote".to_string(),
            ]
        } else {
            vec![
                "/quote".to_string(),
                "/swap".to_string(),
            ]
        };

        let features = if version == "v2" {
            vec![
                "RTSE".to_string(),
                "Jupiter Beam".to_string(),
                "Gasless swaps".to_string(),
                "Multi-router competition".to_string(),
                "JupiterZ RFQ".to_string(),
            ]
        } else {
            vec!["Basic routing".to_string()]
        };

        let migration_required = version != "v2";

        JupiterApiVersion {
            version: version.to_string(),
            base_url,
            endpoints,
            features,
            migration_required,
            deprecation_deadline: if migration_required {
                Some(self.config.deprecation_deadline.clone())
            } else {
                None
            },
        }
    }

    /// Scheme + host of the configured API URL.
    ///
    /// Split-based derivation ("https:///api.jup.ag" triple-slash bug) is
    /// avoided: `scheme://host` is extracted explicitly.
    fn base_url(&self) -> String {
        let scheme = if self.config.api_url.starts_with("https://") {
            "https"
        } else if self.config.api_url.starts_with("http://") {
            "http"
        } else {
            // No scheme — return the input unchanged (best effort).
            return self.config.api_url.clone();
        };
        let rest = self
            .config
            .api_url
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let host = rest.split('/').next().unwrap_or("");
        format!("{}://{}", scheme, host)
    }

    /// Get Jupiter skills integration status
    pub fn get_skills_status(&self) -> JupiterSkillsStatus {
        let api_info = self.get_api_version_info();

        let available_skills = vec![
            "jupiter-swap-migration".to_string(),
            "integrating-jupiter".to_string(),
            "jupiter-lend".to_string(),
            "jupiter-vrfd".to_string(),
        ];

        let mut recommendations = vec
![
            "Use integrating-jupiter skill for best practices validation".to_string(),
        ];

        if api_info.migration_required {
            recommendations.push("Use jupiter-swap-migration skill for structured v2 migration".to_string());
        }

        recommendations.push("Consider jupiter-lend for future lending protocol features".to_string());
        recommendations.push("Use jupiter-vrfd for enhanced token verification".to_string());

        let mut best_practice_compliance = HashMap::new();
        best_practice_compliance.insert("api_key_authentication".to_string(), self.config.api_key.is_some());
        best_practice_compliance.insert("v2_api_usage".to_string(), !api_info.migration_required);
        best_practice_compliance.insert("rtse_enabled".to_string(), self.config.enable_rtse);

        // Price API: the price cache always requests the v3 endpoint
        // (`{price_api_url}/v3?ids=...`) unless the base URL explicitly
        // targets v2.
        let price_api = if self.config.price_api_url.contains("/v2")
            || self.config.price_api_url.contains("price/v2")
        {
            PriceApiStatus::V2
        } else {
            PriceApiStatus::V3
        };

        let migration_status = MigrationStatus {
            swap_api: if self.config.use_swap_v2 {
                SwapApiStatus::V2
            } else {
                SwapApiStatus::V1
            },
            price_api,
            // Primary pricing migrated to the Price API v3; lite-api remains
            // only as a rate-limit fallback in price_cache.
            lite_api: LiteApiStatus::Migrated,
            required_migrations: if !self.config.use_swap_v2 {
                vec!["Swap API v2 migration".to_string()]
            } else {
                vec![]
            },
            optional_migrations: vec![
                "Jupiter Lend integration".to_string(),
                "VRFD token verification".to_string(),
                "Jupiter Beam optimization".to_string(),
            ],
        };

        JupiterSkillsStatus {
            available_skills,
            recommendations,
            migration_status,
            best_practice_compliance,
        }
    }

    /// Validate Jupiter integration against best practices.
    ///
    /// Returns a list of issues/warnings (empty when fully compliant); the
    /// function cannot fail, so no Result wrapper.
    pub fn validate_best_practices(&self) -> Vec<String> {
        let mut issues = vec![];
        let mut warnings = vec![];

        // Check API key authentication
        if self.config.api_key.is_none() {
            warnings.push("API key not configured - keyless access being phased out".to_string());
        }

        // Check API version — only an explicit v1 URL is "deprecated"; an
        // unknown URL gets its own message instead of a misleading v1 claim.
        if self.config.api_url.contains("/v1") {
            issues.push("Using deprecated Swap API v1 - migrate to v2 for new features".to_string());
        } else if !self.config.api_url.contains("/v2") {
            warnings.push(format!(
                "Cannot determine Swap API version from api_url '{}' — expected a /v1 or /v2 URL",
                self.config.api_url
            ));
        }

        // Check RTSE enablement
        if !self.config.enable_rtse && self.config.use_swap_v2 {
            warnings.push("RTSE not enabled - missing automatic slippage optimization".to_string());
        }

        // Return combined issues and warnings
        let all_issues: Vec<String> = issues.into_iter().chain(warnings).collect();

        if all_issues.is_empty() {
            vec!["Jupiter integration follows best practices".to_string()]
        } else {
            all_issues
        }
    }

    /// Get migration recommendations using Jupiter skills
    pub fn get_migration_recommendations(&self) -> Vec<String> {
        let mut recommendations = vec![];

        let api_info = self.get_api_version_info();

        if api_info.migration_required {
            recommendations.push(format!(
                "Migrate Swap API from {} to v2 using jupiter-swap-migration skill",
                api_info.version
            ));
            recommendations.push("Benefits: RTSE, Jupiter Beam, gasless swaps, better pricing".to_string());
        }

        if let Some(deadline) = api_info.deprecation_deadline {
            recommendations.push(format!(
                "Complete migration before deadline: {}",
                deadline
            ));
        }

        recommendations.push("Use integrating-jupiter skill for implementation guidance".to_string());
        recommendations.push("Leverage jupiter-vrfd for enhanced token safety verification".to_string());

        recommendations
    }

    /// Report whether the Jupiter skill files are assumed to be installed.
    ///
    /// This is a STATIC assumption — it does not verify the skills on disk or
    /// in any registry; it returns `true` for every known skill. Callers must
    /// not treat this as a genuine availability probe.
    pub fn check_skills_availability(&self) -> AppResult<HashMap<String, bool>> {
        let mut skills_status = HashMap::new();

        // Static assumption: skills are considered available based on the
        // documented setup, not verified against the environment.
        skills_status.insert(
            "jupiter-swap-migration".to_string(),
            true,
        );

        skills_status.insert(
            "integrating-jupiter".to_string(),
            true,
        );

        skills_status.insert(
            "jupiter-lend".to_string(),
            true,
        );

        skills_status.insert(
            "jupiter-vrfd".to_string(),
            true,
        );

        Ok(skills_status)
    }

    /// Get integration opportunities using Jupiter skills
    pub fn get_integration_opportunities(&self) -> Vec<String> {
        vec
![
            "Use integrating-jupiter for advanced routing strategies".to_string(),
            "Implement jupiter-lend for lending protocol arbitrage".to_string(),
            "Integrate jupiter-vrfd for real-time token verification".to_string(),
            "Leverage Jupiter Beam for MEV protection in high-frequency trading".to_string(),
            "Use RTSE for dynamic slippage optimization in volatile markets".to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_api_version_info_v2() {
        let config = JupiterConfig {
            api_url: "https://api.jup.ag/swap/v2".to_string(),
            use_swap_v2: true,
            enable_rtse: true,
            ..Default::default()
        };

        let integration = JupiterSkillsIntegration::new(config);
        let info = integration.get_api_version_info();

        assert_eq!(info.version, "v2");
        assert_eq!(info.base_url, "https://api.jup.ag");
        assert!(info.endpoints.contains(&"/order".to_string()));
        assert!(info.features.contains(&"RTSE".to_string()));
        assert!(!info.migration_required);
    }

    #[test]
    fn test_get_api_version_info_v1() {
        let config = JupiterConfig {
            api_url: "https://api.jup.ag/swap/v1".to_string(),
            use_swap_v2: false,
            enable_rtse: false,
            ..Default::default()
        };

        let integration = JupiterSkillsIntegration::new(config);
        let info = integration.get_api_version_info();

        assert_eq!(info.version, "v1");
        assert!(info.migration_required);
        assert!(info.deprecation_deadline.is_some());
    }

    #[test]
    fn test_validate_best_practices_compliant() {
        let config = JupiterConfig {
            api_url: "https://api.jup.ag/swap/v2".to_string(),
            use_swap_v2: true,
            enable_rtse: true,
            api_key: Some("test_key".to_string()),
            ..Default::default()
        };

        let integration = JupiterSkillsIntegration::new(config);
        let validation = integration.validate_best_practices();

        assert!(validation.iter().any(|v| v.contains("best practices")));
    }

    #[test]
    fn test_validate_best_practices_non_compliant() {
        let config = JupiterConfig {
            api_url: "https://api.jup.ag/swap/v1".to_string(),
            use_swap_v2: false,
            enable_rtse: false,
            api_key: None,
            ..Default::default()
        };

        let integration = JupiterSkillsIntegration::new(config);
        let validation = integration.validate_best_practices();

        assert!(validation.iter().any(|v| v.contains("deprecated") || v.contains("API key")));
    }

    #[test]
    fn test_get_migration_recommendations() {
        let config = JupiterConfig {
            api_url: "https://api.jup.ag/swap/v1".to_string(),
            use_swap_v2: false,
            ..Default::default()
        };

        let integration = JupiterSkillsIntegration::new(config);
        let recommendations = integration.get_migration_recommendations();

        assert!(!recommendations.is_empty());
        assert!(recommendations.iter().any(|r| r.contains("Migrate")));
        assert!(recommendations.iter().any(|r| r.contains("deadline")));
    }

    #[test]
    fn test_get_integration_opportunities() {
        let config = JupiterConfig::default();
        let integration = JupiterSkillsIntegration::new(config);
        let opportunities = integration.get_integration_opportunities();

        assert!(!opportunities.is_empty());
        assert!(opportunities.len() >= 5);
    }

    #[test]
    fn test_get_skills_status() {
        let config = JupiterConfig {
            api_url: "https://api.jup.ag/swap/v2".to_string(),
            use_swap_v2: true,
            enable_rtse: true,
            ..Default::default()
        };

        let integration = JupiterSkillsIntegration::new(config);
        let status = integration.get_skills_status();

        assert!(!status.available_skills.is_empty());
        assert!(!status.recommendations.is_empty());
        assert_eq!(status.migration_status.swap_api, SwapApiStatus::V2);
        assert!(status.best_practice_compliance.get("v2_api_usage").unwrap_or(&false));
    }
}