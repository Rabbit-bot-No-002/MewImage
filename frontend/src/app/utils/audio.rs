use std::cell::RefCell;

thread_local! {
    static GENERATION_AUDIO_CONTEXT: RefCell<Option<web_sys::AudioContext>> = const { RefCell::new(None) };
}

pub(crate) fn prepare_generation_notification_audio() {
    GENERATION_AUDIO_CONTEXT.with(|slot| {
        let mut context = slot.borrow_mut();
        if context.is_none() {
            *context = web_sys::AudioContext::new().ok();
        }
        if let Some(context) = context.as_ref() {
            let _ = context.resume();
        }
    });
}

pub(crate) fn play_generation_notification(success: bool) {
    GENERATION_AUDIO_CONTEXT.with(|slot| {
        let context = slot.borrow();
        let Some(context) = context.as_ref() else {
            return;
        };
        let _ = context.resume();
        let Ok(oscillator) = context.create_oscillator() else {
            return;
        };
        let Ok(gain) = context.create_gain() else {
            return;
        };
        if oscillator.connect_with_audio_node(&gain).is_err()
            || gain
                .connect_with_audio_node(&context.destination())
                .is_err()
        {
            return;
        }

        let now = context.current_time();
        let (first_frequency, second_frequency, oscillator_type) = if success {
            (659.25, 880.0, web_sys::OscillatorType::Sine)
        } else {
            (440.0, 329.63, web_sys::OscillatorType::Triangle)
        };
        oscillator.set_type(oscillator_type);
        let _ = oscillator
            .frequency()
            .set_value_at_time(first_frequency, now);
        let _ = oscillator
            .frequency()
            .set_value_at_time(second_frequency, now + 0.14);
        let _ = gain.gain().set_value_at_time(0.0001, now);
        let _ = gain.gain().linear_ramp_to_value_at_time(0.09, now + 0.025);
        let _ = gain.gain().linear_ramp_to_value_at_time(0.0001, now + 0.38);
        let _ = oscillator.start_with_when(now);
        let _ = oscillator.stop_with_when(now + 0.4);
    });
}
