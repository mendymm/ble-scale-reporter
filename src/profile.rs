/// Biological sex input used by the scale's body-composition algorithm.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Sex {
    Female,
    Male,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct DateOfBirth {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

/// The calculation mode requested from the scale for one weigh-in.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BodyCompositionAlgorithm {
    /// Complete the weigh-in without calculating body composition.
    WeightOnly,
    Algorithm3,
    Algorithm4,
}

impl BodyCompositionAlgorithm {
    /// Every algorithm value understood by this scale revision.
    ///
    /// Only one value can be sent for a given weigh-in. Algorithms 3 and 4
    /// are alternative body-composition formulas; WeightOnly disables that
    /// calculation entirely.
    pub const ALL: [Self; 3] = [Self::WeightOnly, Self::Algorithm3, Self::Algorithm4];
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuestProfile {
    pub sex: Sex,
    pub date_of_birth: DateOfBirth,
    pub height_cm: f32,
    pub athlete: bool,
    pub algorithm: BodyCompositionAlgorithm,
    pub last_weight_kg: Option<f32>,
}

impl GuestProfile {
    /// Return copies of this profile configured for each supported algorithm.
    /// These must be used on separate weigh-ins, not sent together.
    pub fn for_all_algorithms(self) -> [Self; 3] {
        BodyCompositionAlgorithm::ALL.map(|algorithm| Self { algorithm, ..self })
    }
}
