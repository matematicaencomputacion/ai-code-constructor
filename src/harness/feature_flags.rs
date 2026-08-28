//! Feature flags opt-in via variables de entorno (default desactivado).

const ENV_AI_AGENT_GAP_GUIDANCE: &str = "AI_AGENT_GAP_GUIDANCE";

/// `true` cuando `AI_AGENT_GAP_GUIDANCE` está en `1`, `true` o `yes` (case-insensitive).
pub fn ai_agent_gap_guidance_enabled_from_env() -> bool {
    std::env::var(ENV_AI_AGENT_GAP_GUIDANCE)
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Gap guidance activo si la sesión lo pide explícitamente o el env flag está encendido.
pub fn ai_agent_gap_guidance_enabled(session_enabled: bool) -> bool {
    session_enabled || ai_agent_gap_guidance_enabled_from_env()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gap_guidance_env_flag_defaults_off() {
        unsafe {
            std::env::remove_var(ENV_AI_AGENT_GAP_GUIDANCE);
        }
        assert!(!ai_agent_gap_guidance_enabled_from_env());
        assert!(!ai_agent_gap_guidance_enabled(false));
    }

    #[test]
    fn gap_guidance_env_flag_recognizes_truthy_values() {
        unsafe {
            std::env::set_var(ENV_AI_AGENT_GAP_GUIDANCE, "true");
        }
        assert!(ai_agent_gap_guidance_enabled_from_env());
        unsafe {
            std::env::set_var(ENV_AI_AGENT_GAP_GUIDANCE, "0");
        }
        assert!(!ai_agent_gap_guidance_enabled_from_env());
        unsafe {
            std::env::remove_var(ENV_AI_AGENT_GAP_GUIDANCE);
        }
    }

    #[test]
    fn gap_guidance_session_override_without_env() {
        unsafe {
            std::env::remove_var(ENV_AI_AGENT_GAP_GUIDANCE);
        }
        assert!(ai_agent_gap_guidance_enabled(true));
    }
}
