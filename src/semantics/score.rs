//! Checked fixed-point conversion at model boundaries.

const PPM_SCALE: u32 = 1_000_000;

/// Convert a unit-interval model score to integer parts per million.
///
/// The conversion decomposes the IEEE-754 value into integer mantissa and
/// exponent components. That keeps the durable representation integer-only
/// without unchecked float-to-integer casts.
pub(crate) fn unit_interval_to_ppm(score: f32) -> u32 {
    if score.is_nan() || score <= 0.0 {
        return 0;
    }
    if score >= 1.0 {
        return PPM_SCALE;
    }

    let bits = score.to_bits();
    let raw_exponent = (bits >> 23) & 0xff;
    if raw_exponent == 0 {
        return 0;
    }
    let exponent = i32::try_from(raw_exponent).unwrap_or(0) - 127;
    let shift = 23_i32.saturating_sub(exponent);
    let Ok(shift) = u32::try_from(shift) else {
        return 0;
    };
    let Some(denominator) = 1_u64.checked_shl(shift) else {
        return 0;
    };
    let mantissa = u64::from((bits & 0x7f_ffff) | (1 << 23));
    let numerator = mantissa.saturating_mul(u64::from(PPM_SCALE));
    let rounded = numerator
        .saturating_add(denominator / 2)
        .checked_div(denominator)
        .unwrap_or(0);
    u32::try_from(rounded).unwrap_or(PPM_SCALE).min(PPM_SCALE)
}

/// Convert integer parts per million to a unit-interval model score.
pub(crate) fn ppm_to_unit_interval(ppm: u32) -> f32 {
    let ppm = ppm.min(PPM_SCALE);
    let thousands = u16::try_from(ppm / 1_000).unwrap_or(1_000);
    let remainder = u16::try_from(ppm % 1_000).unwrap_or(0);
    (f32::from(thousands) * 1_000.0 + f32::from(remainder)) / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point_conversion_clamps_and_rounds() {
        assert_eq!(unit_interval_to_ppm(f32::NAN), 0);
        assert_eq!(unit_interval_to_ppm(-1.0), 0);
        assert_eq!(unit_interval_to_ppm(0.0), 0);
        assert_eq!(unit_interval_to_ppm(0.5), 500_000);
        assert_eq!(unit_interval_to_ppm(1.0), 1_000_000);
        assert_eq!(unit_interval_to_ppm(2.0), 1_000_000);
        assert!((ppm_to_unit_interval(500_000) - 0.5).abs() < f32::EPSILON);
        assert!((ppm_to_unit_interval(2_000_000) - 1.0).abs() < f32::EPSILON);
    }
}
