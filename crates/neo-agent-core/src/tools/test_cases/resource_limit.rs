use super::*;

#[test]
fn resource_limit_messages_name_observation_and_next_step() {
    let process = ResourceLimitDetail {
        cause: ResourceLimitCause::ProcessCount,
        configured: Some(32),
        observed: Some(41),
    };
    let memory = ResourceLimitDetail {
        cause: ResourceLimitCause::TreeMemory,
        configured: Some(25),
        observed: Some(31),
    };
    let sampler = ResourceLimitDetail {
        cause: ResourceLimitCause::SamplerUnavailable,
        configured: None,
        observed: None,
    };
    assert!(format_resource_limit(Some(&process)).contains("41 > 32"));
    assert!(format_resource_limit(Some(&memory)).contains("31% > 25%"));
    let sampler = format_resource_limit(Some(&sampler));
    assert!(sampler.contains("monitoring unavailable"));
    assert!(sampler.contains("retry"));
}
