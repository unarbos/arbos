/// Test transport observations only; decisions run through the literal production function above.
pub fn observe(initial: &str, events: &[u8]) -> serde_json::Value {
    let (mut delivery, mut receiver) = match initial {
        "inline" => {
            let (sender, receiver) = oneshot::channel();
            (WaitDelivery::Inline(sender), Some(receiver))
        }
        "polled" => (WaitDelivery::Polled, None),
        _ => (WaitDelivery::Deferred, None),
    };
    let mut steps = Vec::new();
    for &event in events {
        let action = match event {
            0 => format!(
                "{:?}",
                transition_delivery(&mut delivery, DeliveryEvent::Park)
            ),
            1 => format!(
                "{:?}",
                transition_delivery(&mut delivery, DeliveryEvent::Decide)
            ),
            2 => {
                receiver.take();
                "ReceiverDropped".into()
            }
            _ => "Observed".into(),
        };
        let (mode, closed) = match &delivery {
            WaitDelivery::Inline(sender) => ("Inline", Some(sender.is_closed())),
            WaitDelivery::Polled => ("Polled", None),
            WaitDelivery::Deferred => ("Deferred", None),
        };
        let receiver = match &mut receiver {
            None => "Absent",
            Some(receiver) => match receiver.try_recv() {
                Err(oneshot::error::TryRecvError::Empty) => "Open",
                Err(oneshot::error::TryRecvError::Closed) => "Closed",
                Ok(_) => "Delivered",
            },
        };
        steps.push(serde_json::json!({"mode":mode,"sender_closed":closed,"receiver":receiver,"action":action}));
    }
    serde_json::json!(steps)
}
