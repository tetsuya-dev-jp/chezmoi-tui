#[derive(Debug, Clone)]
pub struct AppConfig {
    pub require_two_step_confirmation: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            require_two_step_confirmation: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values_are_safe() {
        let cfg = AppConfig::default();
        assert!(cfg.require_two_step_confirmation);
    }
}
