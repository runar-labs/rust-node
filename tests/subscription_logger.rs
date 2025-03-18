use chrono::Local;

/// Log subscription-related events with timestamps and color coding
pub fn log_subscription(service_name: &str, event_type: &str, message: &str) {
    let timestamp = Local::now().format("%H:%M:%S%.3f").to_string();
    let prefix = format!("[{}][{}][{}]", timestamp, service_name, event_type);

    println!("{} {}", prefix, message);
}

/// Log when ensure_subscriptions is called
pub fn log_ensure_subscriptions(service_name: &str, setup_status: bool) {
    let status = if setup_status {
        "already setup"
    } else {
        "not yet setup"
    };
    log_subscription(
        service_name,
        "ENSURE",
        &format!("ensure_subscriptions called, subscriptions {}", status),
    );
}

/// Log when setup_subscriptions is called
pub fn log_setup_subscriptions(service_name: &str) {
    log_subscription(service_name, "SETUP", "setup_subscriptions called");
}

/// Log when a subscription is registered
pub fn log_subscribe(service_name: &str, topic: &str) {
    log_subscription(
        service_name,
        "SUBSCRIBE",
        &format!("Subscribing to topic '{}'", topic),
    );
}

/// Log when an event callback is triggered
pub fn log_callback(service_name: &str, topic: &str) {
    log_subscription(
        service_name,
        "CALLBACK",
        &format!("Callback triggered for topic '{}'", topic),
    );
}

/// Log when an event is published
pub fn log_event_published(service_name: &str, topic: &str) {
    log_subscription(
        service_name,
        "EVENT",
        &format!("Event published to topic '{}'", topic),
    );
}

/// Log when an error occurs
pub fn log_error(service_name: &str, message: &str) {
    log_subscription(service_name, "ERROR", message);
}
