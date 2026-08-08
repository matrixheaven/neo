//! Multi-agent behavior: background task management, delegate roles, runtime
//! lifecycle, progress, event routing, usage, cancellation, and swarm
//! scheduling (Task 7 structural move).

#[path = "multi_agent_behavior/background.rs"]
mod background;
#[path = "multi_agent_behavior/background_interrupt.rs"]
mod background_interrupt;
#[path = "multi_agent_behavior/background_messaging.rs"]
mod background_messaging;
#[path = "multi_agent_behavior/background_task_stop.rs"]
mod background_task_stop;
#[path = "multi_agent_behavior/cancellation.rs"]
mod cancellation;
#[path = "multi_agent_behavior/event_routing.rs"]
mod event_routing;
#[path = "multi_agent_behavior/lifecycle.rs"]
mod lifecycle;
#[path = "multi_agent_behavior/model_visible_results.rs"]
mod model_visible_results;
#[path = "multi_agent_behavior/progress.rs"]
mod progress;
#[path = "multi_agent_behavior/roles.rs"]
mod roles;
#[path = "multi_agent_behavior/scheduler.rs"]
mod scheduler;
#[path = "multi_agent_behavior/usage.rs"]
mod usage;
