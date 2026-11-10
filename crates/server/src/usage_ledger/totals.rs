use devo_protocol::native::usage::{Money, UsageCallOutcome, UsageField, UsageRecord, UsageTotals};

pub(super) fn record_totals(record: &UsageRecord) -> UsageTotals {
    let mut totals = UsageTotals {
        unreported_fields: Some(
            record
                .usage
                .as_ref()
                .and_then(|usage| usage.unreported_fields.clone())
                .unwrap_or_else(|| UsageField::ALL.to_vec()),
        ),
        call_count: 1,
        failed_call_count: u32::from(record.outcome == UsageCallOutcome::Failed),
        cancelled_call_count: u32::from(record.outcome == UsageCallOutcome::Cancelled),
        estimated_cost: record.estimated_cost.clone(),
        ..UsageTotals::default()
    };
    if let Some(usage) = record.usage.as_ref() {
        totals.total_tokens = usage
            .total_tokens
            .unwrap_or_else(|| usage.input_tokens.saturating_add(usage.output_tokens));
        totals.input_tokens = usage.input_tokens;
        totals.output_tokens = usage.output_tokens;
        totals.reasoning_tokens = usage.reasoning_tokens;
        totals.cache_read_input_tokens = usage.cache_read_input_tokens;
        totals.cache_creation_input_tokens = usage.cache_creation_input_tokens;
        totals.metered_call_count = 1;
    }
    totals
}

pub(super) fn add_totals(total: &mut UsageTotals, usage: &UsageTotals) {
    // A partial price is not the session's cost. Unknown or mixed currencies
    // remain unknown instead of presenting a fabricated zero/partial bill.
    total.estimated_cost = if *total == UsageTotals::default() {
        usage.estimated_cost.clone()
    } else {
        match (&total.estimated_cost, &usage.estimated_cost) {
            (Some(a), Some(b)) if a.currency == b.currency => Some(Money {
                currency: a.currency.clone(),
                micros: a.micros.saturating_add(b.micros),
            }),
            _ => None,
        }
    };
    let mut missing = total
        .unreported_fields
        .take()
        .unwrap_or_else(|| UsageField::ALL.to_vec());
    missing.extend(
        usage
            .unreported_fields
            .clone()
            .unwrap_or_else(|| UsageField::ALL.to_vec()),
    );
    missing.sort();
    missing.dedup();
    total.unreported_fields = Some(missing);
    total.total_tokens = total.total_tokens.saturating_add(usage.total_tokens);
    total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
    total.reasoning_tokens = total
        .reasoning_tokens
        .saturating_add(usage.reasoning_tokens);
    total.cache_read_input_tokens = total
        .cache_read_input_tokens
        .saturating_add(usage.cache_read_input_tokens);
    total.cache_creation_input_tokens = total
        .cache_creation_input_tokens
        .saturating_add(usage.cache_creation_input_tokens);
    total.call_count = total.call_count.saturating_add(usage.call_count);
    total.metered_call_count = total
        .metered_call_count
        .saturating_add(usage.metered_call_count);
    total.failed_call_count = total
        .failed_call_count
        .saturating_add(usage.failed_call_count);
    total.cancelled_call_count = total
        .cancelled_call_count
        .saturating_add(usage.cancelled_call_count);
}
