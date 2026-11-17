use super::*;

fn utc(raw: &str) -> DateTime<Utc> {
    raw.parse().unwrap()
}

fn daily(time: &str, timezone: &str) -> Schedule {
    Schedule {
        id: "morning".into(),
        timing_revision: 1,
        enabled: true,
        timing: Timing::Daily {
            time: time.into(),
            timezone: timezone.into(),
        },
        missed_run_policy: MissedRunPolicy::CatchUp,
    }
}

fn times(plan: &Plan) -> Vec<DateTime<Utc>> {
    plan.occurrences.iter().map(|o| o.scheduled_for).collect()
}

#[test]
fn daily_uses_the_explicit_timezone_and_includes_now() {
    let schedule = daily("09:00", "Asia/Taipei");
    let before = utc("2026-10-15T00:59:59Z");
    let due = utc("2026-10-15T01:00:00Z");
    assert!(schedule
        .plan(before, before)
        .unwrap()
        .occurrences
        .is_empty());
    let plan = schedule.plan(before, due).unwrap();
    assert_eq!(times(&plan), vec![due]);
    assert_eq!(plan.checked_through, due);
    assert_eq!(plan.expired_before, None);
    assert_eq!(schedule.next_after(before).unwrap(), Some(due));
    assert_eq!(
        schedule.next_after(due).unwrap(),
        Some(utc("2026-10-16T01:00:00Z"))
    );
}

#[test]
fn persisted_checkpoint_prevents_replay_and_clock_rollback_does_not_rewind_it() {
    let schedule = daily("09:00", "UTC");
    let first = schedule
        .plan(utc("2026-10-14T00:00:00Z"), utc("2026-10-15T10:00:00Z"))
        .unwrap();
    let repeated = schedule
        .plan(first.checked_through, first.checked_through)
        .unwrap();
    assert!(repeated.occurrences.is_empty());
    let rollback = schedule
        .plan(first.checked_through, utc("2026-10-14T10:00:00Z"))
        .unwrap();
    assert!(rollback.occurrences.is_empty());
    assert_eq!(rollback.checked_through, first.checked_through);
}

#[test]
fn sleep_catches_up_in_order_or_coalesces_to_latest() {
    let mut schedule = daily("09:00", "UTC");
    let start = utc("2026-10-12T09:00:00Z");
    let now = utc("2026-10-15T10:00:00Z");
    assert_eq!(
        times(&schedule.plan(start, now).unwrap()),
        vec![
            utc("2026-10-13T09:00:00Z"),
            utc("2026-10-14T09:00:00Z"),
            utc("2026-10-15T09:00:00Z"),
        ]
    );
    schedule.missed_run_policy = MissedRunPolicy::RunOnce;
    let plan = schedule.plan(start, now).unwrap();
    assert_eq!(times(&plan), vec![utc("2026-10-15T09:00:00Z")]);
    assert_eq!(plan.checked_through, now);
    assert_eq!(plan.expired_before, None);
}

#[test]
fn catch_up_expires_old_work_but_includes_the_exact_seven_day_boundary() {
    let schedule = daily("09:00", "UTC");
    let now = utc("2026-10-15T09:00:00Z");
    let plan = schedule.plan(utc("2020-01-01T00:00:00Z"), now).unwrap();
    assert_eq!(plan.expired_before, Some(utc("2026-10-08T09:00:00Z")));
    assert_eq!(plan.occurrences.len(), 8);
    assert_eq!(times(&plan).first(), plan.expired_before.as_ref());
    assert_eq!(times(&plan).last(), Some(&now));
    assert_eq!(plan.checked_through, now);
}

#[test]
fn run_once_handles_years_of_missed_time_without_replaying_history() {
    let mut schedule = daily("09:00", "UTC");
    schedule.missed_run_policy = MissedRunPolicy::RunOnce;
    let plan = schedule
        .plan(utc("1900-01-01T00:00:00Z"), utc("2026-10-15T08:00:00Z"))
        .unwrap();
    assert_eq!(times(&plan), vec![utc("2026-10-14T09:00:00Z")]);
}

#[test]
fn weekly_uses_iso_weekdays_across_local_date_and_year_boundaries() {
    let mut schedule = daily("09:00", "Pacific/Kiritimati");
    schedule.timing = Timing::Weekly {
        days: vec![7, 1],
        time: "09:00".into(),
        timezone: "Pacific/Kiritimati".into(),
    };
    // Sunday Jan 4 at 09:00 locally is Saturday Jan 3 at 19:00 UTC.
    assert_eq!(
        schedule.next_after(utc("2025-12-30T00:00:00Z")).unwrap(),
        Some(utc("2026-01-03T19:00:00Z"))
    );
    let plan = schedule
        .plan(utc("2026-01-03T18:00:00Z"), utc("2026-01-04T19:00:00Z"))
        .unwrap();
    assert_eq!(
        times(&plan),
        vec![utc("2026-01-03T19:00:00Z"), utc("2026-01-04T19:00:00Z")]
    );
}

#[test]
fn spring_dst_gap_runs_at_first_valid_instant() {
    let schedule = daily("02:30", "America/New_York");
    let start = utc("2026-03-07T08:00:00Z");
    let end = utc("2026-03-09T07:00:00Z");
    assert_eq!(
        times(&schedule.plan(start, end).unwrap()),
        vec![utc("2026-03-08T07:00:00Z"), utc("2026-03-09T06:30:00Z")]
    );
    assert_eq!(
        schedule.next_after(start).unwrap(),
        Some(utc("2026-03-08T07:00:00Z"))
    );
}

#[test]
fn weekly_gap_forward_and_backward_search_agree() {
    let mut schedule = daily("02:30", "America/New_York");
    schedule.timing = Timing::Weekly {
        days: vec![7],
        time: "02:30".into(),
        timezone: "America/New_York".into(),
    };
    schedule.missed_run_policy = MissedRunPolicy::RunOnce;
    let start = utc("2026-02-01T00:00:00Z");
    let end = utc("2026-03-09T00:00:00Z");
    assert_eq!(
        times(&schedule.plan(start, end).unwrap()),
        vec![utc("2026-03-08T07:00:00Z")]
    );
    assert_eq!(
        schedule.next_after(utc("2026-03-01T07:30:00Z")).unwrap(),
        Some(utc("2026-03-08T07:00:00Z"))
    );
}

#[test]
fn fall_dst_overlap_runs_at_first_instant_only() {
    let schedule = daily("01:30", "America/New_York");
    let start = utc("2026-11-01T00:00:00Z");
    let end = utc("2026-11-01T07:00:00Z");
    assert_eq!(
        times(&schedule.plan(start, end).unwrap()),
        vec![utc("2026-11-01T05:30:00Z")]
    );
    assert_eq!(
        schedule.next_after(utc("2026-11-01T05:30:00Z")).unwrap(),
        Some(utc("2026-11-02T06:30:00Z"))
    );
    assert!(schedule
        .plan(utc("2026-11-01T05:30:00Z"), end)
        .unwrap()
        .occurrences
        .is_empty());
}

#[test]
fn once_runs_at_boundary_and_then_is_exhausted() {
    let mut schedule = daily("09:00", "UTC");
    let at = utc("2026-10-15T09:00:00Z");
    schedule.timing = Timing::Once { at };
    let before = utc("2026-10-15T08:00:00Z");
    assert_eq!(schedule.next_after(before).unwrap(), Some(at));
    assert!(schedule
        .plan(before, before)
        .unwrap()
        .occurrences
        .is_empty());
    assert_eq!(times(&schedule.plan(before, at).unwrap()), vec![at]);
    assert!(schedule
        .plan(at, utc("2026-10-16T00:00:00Z"))
        .unwrap()
        .occurrences
        .is_empty());
    assert_eq!(schedule.next_after(at).unwrap(), None);
}

#[test]
fn overdue_one_time_task_obeys_missed_run_policy() {
    let mut schedule = daily("09:00", "UTC");
    schedule.timing = Timing::Once {
        at: utc("2026-01-01T00:00:00Z"),
    };
    let start = utc("2025-12-01T00:00:00Z");
    let now = utc("2026-10-15T00:00:00Z");
    assert!(schedule.plan(start, now).unwrap().occurrences.is_empty());
    schedule.missed_run_policy = MissedRunPolicy::RunOnce;
    assert_eq!(
        times(&schedule.plan(start, now).unwrap()),
        vec![utc("2026-01-01T00:00:00Z")]
    );
}

#[test]
fn disabled_schedule_has_no_due_or_future_work_and_accounts_for_disabled_time() {
    let mut schedule = daily("09:00", "UTC");
    schedule.enabled = false;
    let now = utc("2026-10-15T10:00:00Z");
    let plan = schedule.plan(utc("2026-10-14T00:00:00Z"), now).unwrap();
    assert!(plan.occurrences.is_empty());
    assert_eq!(plan.checked_through, now);
    assert_eq!(schedule.next_after(now).unwrap(), None);
    schedule.enabled = true;
    assert!(schedule
        .plan(plan.checked_through, now)
        .unwrap()
        .occurrences
        .is_empty());
}

#[test]
fn invalid_time_zone_and_weekdays_are_rejected() {
    let now = utc("2026-10-15T00:00:00Z");
    for time in ["24:00", "9:00", "09:0", "09:00:00", "09:60", ""] {
        assert!(
            matches!(
                daily(time, "UTC").plan(now, now),
                Err(ScheduleError::InvalidTime(_))
            ),
            "{time}"
        );
    }
    assert!(matches!(
        daily("09:00", "Mars/Olympus").next_after(now),
        Err(ScheduleError::InvalidTimezone(_))
    ));
    for days in [vec![], vec![0], vec![8], vec![1, 1]] {
        let mut schedule = daily("09:00", "UTC");
        schedule.timing = Timing::Weekly {
            days,
            time: "09:00".into(),
            timezone: "UTC".into(),
        };
        assert_eq!(
            schedule.next_after(now),
            Err(ScheduleError::InvalidWeekdays)
        );
    }
    let mut schedule = daily("09:00", "UTC");
    schedule.id = " ".into();
    assert_eq!(schedule.plan(now, now), Err(ScheduleError::EmptyId));
}

#[test]
fn identity_is_stable_on_replay_and_changes_with_timing_revision() {
    let mut schedule = daily("09:00", "UTC");
    let start = utc("2026-10-15T08:00:00Z");
    let end = utc("2026-10-15T10:00:00Z");
    let first = schedule.plan(start, end).unwrap();
    assert_eq!(first, schedule.plan(start, end).unwrap());
    schedule.timing_revision += 1;
    assert_ne!(
        first.occurrences,
        schedule.plan(start, end).unwrap().occurrences
    );
}

#[test]
fn bridge_shaped_fixture_deserializes_and_produces_utc_occurrences() {
    let schedule: Schedule = serde_json::from_value(serde_json::json!({
        "id": "weekday-summary", "timing_revision": 2, "enabled": true,
        "timing": { "kind": "weekly", "days": [1, 2, 3, 4, 5], "time": "09:00", "timezone": "Asia/Taipei" },
        "missed_run_policy": "run_once"
    })).unwrap();
    let plan = schedule
        .plan(utc("2026-10-14T01:00:00Z"), utc("2026-10-15T01:00:00Z"))
        .unwrap();
    assert_eq!(
        serde_json::to_value(&plan.occurrences[0]).unwrap(),
        serde_json::json!({
            "schedule_id": "weekday-summary", "timing_revision": 2, "scheduled_for": "2026-10-15T01:00:00Z"
        })
    );
}

#[test]
fn out_of_range_local_dates_return_errors_instead_of_panicking() {
    assert_eq!(
        daily("09:00", "Pacific/Kiritimati").next_after(DateTime::<Utc>::MAX_UTC),
        Err(ScheduleError::DateRange)
    );
    assert_eq!(
        daily("09:00", "UTC").next_after(DateTime::<Utc>::MAX_UTC),
        Err(ScheduleError::DateRange)
    );
    let mut schedule = daily("09:00", "America/New_York");
    schedule.missed_run_policy = MissedRunPolicy::RunOnce;
    assert_eq!(
        schedule.plan(DateTime::<Utc>::MIN_UTC, utc("2026-10-15T00:00:00Z")),
        Err(ScheduleError::DateRange)
    );
}

#[test]
fn monthly_skips_missing_dates_and_supports_multiple_days() {
    let mut schedule = daily("09:00", "Asia/Taipei");
    schedule.timing = Timing::Monthly {
        days: vec![15, 31],
        time: "09:00".into(),
        timezone: "Asia/Taipei".into(),
    };
    assert_eq!(
        schedule.next_after(utc("2026-01-31T01:00:00Z")).unwrap(),
        Some(utc("2026-02-15T01:00:00Z"))
    );
    assert_eq!(
        times(
            &schedule
                .plan(utc("2026-02-14T01:00:00Z"), utc("2026-02-16T01:00:00Z"))
                .unwrap()
        ),
        vec![utc("2026-02-15T01:00:00Z")]
    );
    schedule.timing = Timing::Monthly {
        days: vec![31],
        time: "09:00".into(),
        timezone: "Asia/Taipei".into(),
    };
    assert_eq!(
        schedule.next_after(utc("2026-01-31T01:00:00Z")).unwrap(),
        Some(utc("2026-03-31T01:00:00Z"))
    );
    for days in [vec![], vec![0], vec![32], vec![1, 1]] {
        schedule.timing = Timing::Monthly {
            days,
            time: "09:00".into(),
            timezone: "UTC".into(),
        };
        assert_eq!(
            schedule.next_after(utc("2026-01-01T00:00:00Z")),
            Err(ScheduleError::InvalidMonthDays)
        );
    }
}
