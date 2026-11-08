//! Immutable localization contexts with bounded live publication. Catalog source is JSON; generated accessors are typed.
use icu_decimal::{input::Decimal, DecimalFormatter};
use icu_locale::Locale;
use icu_plurals::{PluralCategory, PluralRules};
use serde::{Deserialize, Deserializer, Serialize};
use std::sync::OnceLock;

mod live;
pub use live::{PrepareError, PreparedLocale, PublishError};

pub const CONTRIBUTE_URL: &str =
    "https://github.com/GLinnik21/plx-native/blob/main/docs/localization.md";

// `Preference`, `Language`, `SHIPPED` and every table over them are generated from
// `locales/languages.json` (see `platform/build_support/catalog.rs`); this file holds only the
// behavior around them. A shipped tag is never renamed: it is what the preferences store.
include!(concat!(env!("OUT_DIR"), "/languages.rs"));
impl<'de> Deserialize<'de> for Preference {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Ok(Self::from_tag(v.as_str().unwrap_or("system")))
    }
}
impl Preference {
    /// Serde's `skip_serializing_if`: System is what an absent preference already means.
    pub fn is_system(&self) -> bool {
        *self == Self::System
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Clock {
    Locale,
    H12,
    H24,
}

pub struct LocaleContext {
    preference: Preference,
    language: Language,
    format_locale: String,
    clock: Clock,
    time: icu_datetime::NoCalendarFormatter<icu_datetime::fieldsets::T>,
    time_minute: icu_datetime::NoCalendarFormatter<icu_datetime::fieldsets::T>,
    plural: PluralRules,
    decimal: DecimalFormatter,
    date: icu_datetime::FixedCalendarDateTimeFormatter<
        icu_calendar::Gregorian,
        icu_datetime::fieldsets::YMD,
    >,
    date_medium: icu_datetime::FixedCalendarDateTimeFormatter<
        icu_calendar::Gregorian,
        icu_datetime::fieldsets::YMD,
    >,
}
impl LocaleContext {
    /// Exercise expanded catalogs without changing the process-wide locale or environment.
    #[cfg(any(test, feature = "test-support"))]
    pub fn pseudo_for_test() -> Self {
        let mut context = Self::resolve(Preference::En, None, None, None, None);
        context.language = Language::Pseudo;
        context
    }

    pub fn resolve(
        preference: Preference,
        ui: Option<&str>,
        fmt: Option<&str>,
        clock: Option<&str>,
        env: Option<&str>,
    ) -> Self {
        Self::try_resolve(preference, ui, fmt, clock, env).expect("compiled locale data")
    }

    fn try_resolve(
        preference: Preference,
        ui: Option<&str>,
        fmt: Option<&str>,
        clock: Option<&str>,
        env: Option<&str>,
    ) -> Result<Self, PrepareError> {
        let selected = if preference == Preference::System {
            ui.and_then(normalize)
                .or_else(|| env.and_then(normalize))
                .unwrap_or_else(|| "en".into())
        } else {
            preference.tag().into()
        };
        let language = Language::parse(&selected);
        let format_locale = fmt
            .and_then(normalize)
            .or_else(|| {
                let selected = selected.parse::<Locale>().ok()?;
                (selected.id.language.as_str() == language.tag() && selected.id.region.is_some())
                    .then(|| selected.to_string())
            })
            .unwrap_or_else(|| language.regional().into());
        let locale = format_locale.parse::<Locale>().map_err(|_| PrepareError::Unavailable)?;
        let decimal = DecimalFormatter::try_new(locale.clone().into(), Default::default())
            .or_else(|_| DecimalFormatter::try_new(Default::default(), Default::default()))
            .map_err(|_| PrepareError::Unavailable)?;
        let date = icu_datetime::FixedCalendarDateTimeFormatter::try_new_unstable(
            &CaptionDateProvider(None),
            locale.clone().into(),
            icu_datetime::fieldsets::YMD::short()
                .with_year_style(icu_datetime::options::YearStyle::Full),
        )
        .or_else(|_| {
            icu_datetime::FixedCalendarDateTimeFormatter::try_new_unstable(
                &CaptionDateProvider(None),
                Default::default(),
                icu_datetime::fieldsets::YMD::short()
                    .with_year_style(icu_datetime::options::YearStyle::Full),
            )
        }).map_err(|_| PrepareError::Unavailable)?;
        // Named months are interface text. A TV may report an English format locale even when
        // Russian was chosen in the app; retain regional order only within the same language.
        let named_locale = if locale.id.language.as_str() == language.tag() { locale.clone() }
            else { language.regional().parse::<Locale>().map_err(|_| PrepareError::Unavailable)? };
        let (months, _) = month_names(language, MonthWidth::Medium);
        let date_medium = icu_datetime::FixedCalendarDateTimeFormatter::try_new_unstable(
            &CaptionDateProvider(Some(months)),
            named_locale.into(),
            icu_datetime::fieldsets::YMD::medium()
                .with_year_style(icu_datetime::options::YearStyle::Full),
        ).or_else(|_| {
            icu_datetime::FixedCalendarDateTimeFormatter::try_new_unstable(
                &CaptionDateProvider(Some(&EN_MEDIUM)),
                Default::default(),
                icu_datetime::fieldsets::YMD::medium()
                    .with_year_style(icu_datetime::options::YearStyle::Full),
            )
        }).map_err(|_| PrepareError::Unavailable)?;
        // The regional locale, not the bare tag: pt-PT counts 0 as plural where bare `pt` (pt-BR) does not.
        let plural =
            PluralRules::try_new_cardinal(language.regional().parse::<Locale>()
                .map_err(|_| PrepareError::Unavailable)?.into())
                .map_err(|_| PrepareError::Unavailable)?;
        let clock = match clock {
            Some("12") => Clock::H12,
            Some("24") => Clock::H24,
            _ => Clock::Locale,
        };
        let mut time_preferences: icu_datetime::DateTimeFormatterPreferences = locale.into();
        time_preferences.hour_cycle = match clock {
            Clock::H12 => Some(icu_datetime::preferences::HourCycle::H12),
            Clock::H24 => Some(icu_datetime::preferences::HourCycle::H23),
            Clock::Locale => time_preferences.hour_cycle,
        };
        let time = icu_datetime::NoCalendarFormatter::try_new(time_preferences, icu_datetime::fieldsets::T::hms())
            .or_else(|_| icu_datetime::NoCalendarFormatter::try_new(Default::default(), icu_datetime::fieldsets::T::hms()))
            .map_err(|_| PrepareError::Unavailable)?;
        let time_minute = icu_datetime::NoCalendarFormatter::try_new(time_preferences, icu_datetime::fieldsets::T::hm())
            .or_else(|_| icu_datetime::NoCalendarFormatter::try_new(Default::default(), icu_datetime::fieldsets::T::hm()))
            .map_err(|_| PrepareError::Unavailable)?;
        Ok(Self {
            preference,
            language,
            format_locale,
            clock,
            time,
            time_minute,
            plural,
            decimal,
            date,
            date_medium,
        })
    }
    pub fn preference(&self) -> Preference {
        self.preference
    }
    pub fn language(&self) -> Language {
        self.language
    }
    pub fn format_locale(&self) -> &str {
        &self.format_locale
    }
    pub fn clock(&self) -> Clock {
        self.clock
    }
    /// Time-of-day from already converted local clock parts; no clock or timezone read.
    pub fn time(&self, parts: [u8; 3]) -> Option<String> {
        let time = icu_datetime::input::Time::try_new(parts[0], parts[1], parts[2], 0).ok()?;
        Some(self.time.format(&time).to_string())
    }

    /// Minute precision for the permanent playback clock; uses the same locale
    /// and hour-cycle preference as the second-precision formatter.
    pub fn time_minute(&self, parts: [u8; 3]) -> Option<String> {
        // ICU accepts leap-second 60; the playback clock uses the normalized
        // 0..=59 contract of local_clock_parts, even when seconds are not shown.
        if parts[2] >= 60 { return None; }
        let time = icu_datetime::input::Time::try_new(parts[0], parts[1], parts[2], 0).ok()?;
        Some(self.time_minute.format(&time).to_string())
    }

    pub fn plural(&self, n: i64) -> PluralCategory {
        self.plural.category_for(n.unsigned_abs())
    }
    pub fn number(&self, n: i64) -> String {
        self.decimal.format(&Decimal::from(n)).to_string()
    }
    pub fn decimal(&self, n: i64, scale: i16) -> String {
        let mut d = Decimal::from(n);
        d.multiply_pow10(-scale);
        d.pad_end(-scale);
        self.decimal.format(&d).to_string()
    }
    pub fn date(&self, year: i32, month: u8, day: u8) -> Option<String> {
        let d = icu_calendar::Date::try_new_gregorian(year, month, day).ok()?;
        Some(self.date.format(&d).to_string())
    }
    /// Full calendar date with an abbreviated month in the interface language. A matching
    /// regional format keeps its order; another language's numeric-format preference cannot
    /// replace the month names.
    pub fn date_medium(&self, year: i32, month: u8, day: u8) -> Option<String> {
        let d = icu_calendar::Date::try_new_gregorian(year, month, day).ok()?;
        Some(self.date_medium.format(&d).to_string())
    }
    /// Day and month with no year, for release captions: "12 Mar" / "12 March" in English,
    /// "12 de marzo" in Spanish, "12 mars" / "12 janv." in French, "12 marzo" in Italian,
    /// "12 de março" in European Portuguese, "12. März" in German, "március 12." in Hungarian, and in Polish, Belarusian and Russian the genitive
    /// after the day ("12 marca", "12 сакавіка", "12 марта") with the abbreviated form for medium. Hand-written tables, not
    /// ICU month data, so no name data is compiled in for the shipped languages. The year is
    /// unknown, so 29 February is a valid day; a day the calendar lacks returns `None`.
    pub fn month_day(&self, month: u8, day: u8, width: MonthWidth) -> Option<String> {
        // Validate against a leap year first: `month - 1` below is only reached for 1..=12.
        icu_calendar::Date::try_new_gregorian(2024, month, day).ok()?;
        let i = usize::from(month - 1);
        // Hungarian puts the month first and the day after it, with the ordinal dot: "március 12.".
        if self.language == Language::Hu {
            let names = match width {
                MonthWidth::Medium => &HU_MEDIUM,
                MonthWidth::Long => &HU_LONG,
            };
            return Some(format!("{} {day}.", names[i]));
        }
        let (names, joiner) = month_names(self.language, width);
        Some(format!("{day}{joiner}{}", names[i]))
    }
}

/// Month data shared by yearless captions and full sort dates. Keep the ICU pattern/number
/// formatter, but do not link its all-locale Gregorian month-name database for these ten tables.
fn month_names(language: Language, width: MonthWidth) -> (&'static [&'static str; 12], &'static str) {
    match (language, width) {
        (Language::En | Language::Pseudo, MonthWidth::Medium) => (&EN_MEDIUM, " "),
        (Language::En | Language::Pseudo, MonthWidth::Long) => (&EN_LONG, " "),
        (Language::Es, MonthWidth::Medium) => (&ES_MEDIUM, " "),
        (Language::Es, MonthWidth::Long) => (&ES_LONG, " de "),
        (Language::Be, MonthWidth::Medium) => (&BE_MEDIUM, " "),
        (Language::Be, MonthWidth::Long) => (&BE_GENITIVE, " "),
        (Language::Ru, MonthWidth::Medium) => (&RU_MEDIUM, " "),
        (Language::Ru, MonthWidth::Long) => (&RU_GENITIVE, " "),
        (Language::Pl, MonthWidth::Medium) => (&PL_MEDIUM, " "),
        (Language::Pl, MonthWidth::Long) => (&PL_GENITIVE, " "),
        (Language::Fr, MonthWidth::Medium) => (&FR_MEDIUM, " "),
        (Language::Fr, MonthWidth::Long) => (&FR_LONG, " "),
        (Language::It, MonthWidth::Medium) => (&IT_MEDIUM, " "),
        (Language::It, MonthWidth::Long) => (&IT_LONG, " "),
        (Language::Pt, MonthWidth::Medium) => (&PT_MEDIUM, " "),
        (Language::Pt, MonthWidth::Long) => (&PT_LONG, " de "),
        (Language::De, MonthWidth::Medium) => (&DE_MEDIUM, ". "),
        (Language::De, MonthWidth::Long) => (&DE_LONG, ". "),
        (Language::Hu, MonthWidth::Medium) => (&HU_MEDIUM, " "),
        (Language::Hu, MonthWidth::Long) => (&HU_LONG, " "),
    }
}

// Short dates are numeric; only medium dates request abbreviated month names.
struct CaptionDateProvider(Option<&'static [&'static str; 12]>);
icu_provider::marker::impl_data_provider_never_marker!(CaptionDateProvider);

macro_rules! forward_caption_date_data {
    ($marker:path, $provider:expr) => {
        impl icu_provider::DataProvider<$marker> for CaptionDateProvider {
            fn load(
                &self,
                req: icu_provider::DataRequest,
            ) -> Result<icu_provider::DataResponse<$marker>, icu_provider::DataError> {
                icu_provider::DataProvider::<$marker>::load(&$provider, req)
            }
        }
    };
}
forward_caption_date_data!(
    icu_datetime::provider::DatetimePatternsDateGregorianV1,
    icu_datetime::provider::Baked
);
forward_caption_date_data!(
    icu_datetime::provider::neo::DatetimeNamesYearGregorianV1,
    icu_datetime::provider::Baked
);
forward_caption_date_data!(
    icu_decimal::provider::DecimalSymbolsV1,
    icu_decimal::provider::Baked
);
forward_caption_date_data!(
    icu_decimal::provider::DecimalDigitsV1,
    icu_decimal::provider::Baked
);

impl icu_provider::DataProvider<icu_datetime::provider::neo::DatetimeNamesMonthGregorianV1>
    for CaptionDateProvider
{
    fn load(
        &self,
        req: icu_provider::DataRequest,
    ) -> Result<
        icu_provider::DataResponse<icu_datetime::provider::neo::DatetimeNamesMonthGregorianV1>,
        icu_provider::DataError,
    > {
        use icu_datetime::provider::neo::{marker_attrs, MonthNames};
        if req.id.marker_attributes != marker_attrs::ABBR {
            return Err(icu_provider::DataErrorKind::IdentifierNotFound.into_error());
        }
        let mut names = *self
            .0
            .ok_or_else(|| icu_provider::DataErrorKind::IdentifierNotFound.into_error())?;
        let locale = req.id.locale;
        match (
            locale.language.as_str(),
            locale.region.as_ref().map(|r| r.as_str()),
        ) {
            ("en", _) => {
                // CLDR English regions inherit either US or world-English month names.
                let fallback =
                    icu_locale::fallback::LocaleFallbacker::new().for_config(Default::default());
                let mut locales = fallback.fallback_for(*locale);
                loop {
                    match locales.get().region.as_ref().map(|r| r.as_str()) {
                        Some("AU") => {
                            names[5] = "June";
                            names[6] = "July";
                            names[8] = "Sept";
                            break;
                        }
                        Some("001") => {
                            names[8] = "Sept";
                            break;
                        }
                        None => break,
                        _ => {
                            locales.step();
                        }
                    }
                }
            }
            ("es", Some("MX")) => names[8] = "sep",
            ("es", Some("PE" | "UY")) => {
                names = [
                    "ene.", "feb.", "mar.", "abr.", "may.", "jun.", "jul.", "ago.", "set.", "oct.",
                    "nov.", "dic.",
                ]
            }
            ("es", Some("PY" | "VE")) => {
                names = [
                    "ene.", "feb.", "mar.", "abr.", "may.", "jun.", "jul.", "ago.", "sept.",
                    "oct.", "nov.", "dic.",
                ]
            }
            ("fr", Some("CA")) => names[6] = "juill.",
            ("fr", Some("MA")) => {
                names[0] = "jan.";
                names[1] = "fév.";
                names[2] = "mar.";
                names[5] = "jui.";
            }
            _ => {}
        }
        Ok(icu_provider::DataResponse {
            metadata: Default::default(),
            payload: icu_provider::DataPayload::from_owned(MonthNames::Linear(
                names.as_slice().into(),
            )),
        })
    }
}

/// Width of a month-and-day caption: `Medium` is the abbreviated month, `Long` the full one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonthWidth {
    Medium,
    Long,
}

/// CLDR `pl` format months: abbreviated without dots, full names in the genitive after a day.
const PL_MEDIUM: [&str; 12] =
    ["sty", "lut", "mar", "kwi", "maj", "cze", "lip", "sie", "wrz", "paź", "lis", "gru"];
const PL_GENITIVE: [&str; 12] = [
    "stycznia", "lutego", "marca", "kwietnia", "maja", "czerwca", "lipca", "sierpnia", "września",
    "października", "listopada", "grudnia",
];

/// CLDR `ru` format months: genitive after a day, including abbreviated "мая".
const RU_MEDIUM: [&str; 12] = [
    "янв.", "февр.", "мар.", "апр.", "мая", "июн.", "июл.", "авг.", "сент.", "окт.", "нояб.", "дек.",
];
const RU_GENITIVE: [&str; 12] = [
    "января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября",
    "ноября", "декабря",
];

/// CLDR `hu` abbreviated format months, with their dots.
const HU_MEDIUM: [&str; 12] =
    ["jan.", "febr.", "márc.", "ápr.", "máj.", "jún.", "júl.", "aug.", "szept.", "okt.", "nov.", "dec."];
const HU_LONG: [&str; 12] = [
    "január", "február", "március", "április", "május", "június", "július", "augusztus", "szeptember",
    "október", "november", "december",
];
const EN_MEDIUM: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const EN_LONG: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September",
    "October", "November", "December",
];
const ES_MEDIUM: [&str; 12] =
    ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic"];
const ES_LONG: [&str; 12] = [
    "enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre",
    "octubre", "noviembre", "diciembre",
];
/// CLDR `be` abbreviated format months: three letters, no dot, and "мая" (the genitive) for May.
const BE_MEDIUM: [&str; 12] =
    ["сту", "лют", "сак", "кра", "мая", "чэр", "ліп", "жні", "вер", "кас", "ліс", "сне"];
/// Genitive, because a day number governs the month in Belarusian ("12 сакавіка").
const BE_GENITIVE: [&str; 12] = [
    "студзеня", "лютага", "сакавіка", "красавіка", "траўня", "чэрвеня", "ліпеня", "жніўня",
    "верасня", "кастрычніка", "лістапада", "снежня",
];
const FR_MEDIUM: [&str; 12] =
    ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."];
const FR_LONG: [&str; 12] = [
    "janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre",
    "octobre", "novembre", "décembre",
];
const IT_MEDIUM: [&str; 12] =
    ["gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic"];
const IT_LONG: [&str; 12] = [
    "gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre",
    "ottobre", "novembre", "dicembre",
];
const PT_MEDIUM: [&str; 12] =
    ["jan.", "fev.", "mar.", "abr.", "mai.", "jun.", "jul.", "ago.", "set.", "out.", "nov.", "dez."];
const PT_LONG: [&str; 12] = [
    "janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro",
    "outubro", "novembro", "dezembro",
];
/// German writes the day as an ordinal: "12. März".
const DE_MEDIUM: [&str; 12] =
    ["Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.", "Nov.", "Dez."];
const DE_LONG: [&str; 12] = [
    "Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober",
    "November", "Dezember",
];
/// Reject control bytes and invalid tags before they can become HTTP headers. POSIX suffixes
/// are only used at this input boundary; callers retain canonical BCP-47 tags.
pub fn normalize(s: &str) -> Option<String> {
    if !s.is_ascii() || s.bytes().any(|b| b.is_ascii_control()) {
        return None;
    }
    let base = s.trim().split(['.', '@']).next()?.replace('_', "-");
    if base.eq_ignore_ascii_case("C") || base.eq_ignore_ascii_case("POSIX") {
        return None;
    }
    let l = base.parse::<Locale>().ok()?;
    (l.id.language.as_str() != "und").then(|| l.to_string())
}
/// `webOS 4.10.2`, or the English "webOS unknown" read-out when the firmware file could not
/// be read — the release is the one field a stranger's report needs, and "unknown" is the honest
/// reading of an empty one rather than a plausible default. Shared by the support lines of the
/// failure read-out and of the sign-in report.
pub fn webos_release_line(info: &crate::tv::device::Info) -> String {
    if info.major == 0 {
        msg::browse_diagnostics_unknown_os().to_string()
    } else {
        format!("webOS {}", info.release)
    }
}
static CURRENT: OnceLock<live::Registry> = OnceLock::new();
static SAVED_PREFERENCE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The confirmed persisted preference. Live publication is a separate application step.
/// Reading it never touches credential storage.
pub fn saved_preference() -> Preference {
    Preference::from_slot(SAVED_PREFERENCE.load(std::sync::atomic::Ordering::Acquire))
}

pub fn set_saved_preference(value: Preference) {
    SAVED_PREFERENCE.store(value.slot(), std::sync::atomic::Ordering::Release);
}

#[cfg(any(test, feature = "test-support"))]
pub fn saved_preference_for_test(value: Preference) -> Preference {
    plx_base::testlock::assert_held("saved language preference");
    let previous = saved_preference();
    set_saved_preference(value);
    previous
}

thread_local! {
    static THREAD_LOCALE: std::cell::Cell<Option<&'static LocaleContext>> = const { std::cell::Cell::new(None) };
}

/// The fixed English context behind every `"translate": false` message: English plural rules and
/// en-US number formatting, whatever the UI language or the TV's format locale. The generated
/// accessors of an English-only message call this instead of [`current`].
pub fn english() -> &'static LocaleContext {
    static ENGLISH: OnceLock<LocaleContext> = OnceLock::new();
    ENGLISH.get_or_init(|| LocaleContext::resolve(Preference::En, None, Some("en-US"), Some("24"), None))
}

/// Run `f` with [`current`] answering [`english`] on THIS thread, then restore what it answered
/// before (so it nests, and nests under the test overrides). It is how the diagnostics panel
/// composes its shared, translated helpers (units, decimals, quality labels) in English.
///
/// **Nothing reached inside `f` may cache localized text process-wide** (a `OnceLock<String>`
/// first touched here would freeze English for the whole app). None does today; keep it so.
pub fn render_in_english<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(Option<&'static LocaleContext>);
    impl Drop for Restore {
        fn drop(&mut self) {
            THREAD_LOCALE.with(|slot| slot.set(self.0));
        }
    }
    let _restore = Restore(THREAD_LOCALE.with(|slot| slot.replace(Some(english()))));
    f()
}

/// Resolve every catalog accessor on THIS test thread in the expanded pseudo-locale until the
/// guard drops. Tests run in parallel, so the process-wide [`current`] must stay English for every
/// other test; a thread-local override is what lets one test draw a whole screen in `[!! … !!]`.
#[cfg(any(test, feature = "test-support"))]
pub fn pseudo_on_this_thread_for_test() -> ThreadLocaleGuard {
    static PSEUDO: OnceLock<LocaleContext> = OnceLock::new();
    let pseudo = PSEUDO.get_or_init(LocaleContext::pseudo_for_test);
    ThreadLocaleGuard(THREAD_LOCALE.with(|slot| slot.replace(Some(pseudo))))
}

/// Resolve every catalog accessor on THIS test thread in one shipped UI language until the guard
/// drops — the pseudo-locale's thread-local door, for tests that measure real translations.
#[cfg(any(test, feature = "test-support"))]
pub fn language_on_this_thread_for_test(preference: Preference) -> ThreadLocaleGuard {
    static CONTEXTS: [OnceLock<LocaleContext>; Language::SHIPPED.len()] =
        [const { OnceLock::new() }; Language::SHIPPED.len()];
    // System resolves to English here, as a test without a platform locale does.
    let slot = &CONTEXTS[preference.language().unwrap_or(Language::En).index()];
    let cx = slot.get_or_init(|| LocaleContext::resolve(preference, None, None, None, None));
    ThreadLocaleGuard(THREAD_LOCALE.with(|slot| slot.replace(Some(cx))))
}

#[cfg(any(test, feature = "test-support"))]
pub struct ThreadLocaleGuard(Option<&'static LocaleContext>);

#[cfg(any(test, feature = "test-support"))]
impl Drop for ThreadLocaleGuard {
    fn drop(&mut self) {
        THREAD_LOCALE.with(|slot| slot.set(self.0));
    }
}

fn registry() -> &'static live::Registry {
    #[cfg(any(test, feature = "test-support"))]
    {
        CURRENT.get_or_init(|| live::Registry::new(Preference::En, live::BootInputs::default())
            .expect("compiled English locale data"))
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        CURRENT.get().expect("locale initialized before any screen or Plex request")
    }
}

pub fn current() -> &'static LocaleContext {
    if let Some(cx) = THREAD_LOCALE.with(std::cell::Cell::get) { return cx; }
    registry().current()
}

/// Shared publication generation, independent of temporary diagnostic/test overrides.
pub fn epoch() -> u64 { registry().epoch() }

/// Resolve compiled data without changing the current or saved preference.
/// Controlled recordings and explicit simulator locale overrides cannot switch live.
pub fn prepare(preference: Preference) -> Result<PreparedLocale, PrepareError> {
    CURRENT.get().ok_or(PrepareError::NotInitialized)?.prepare(preference)
}

/// Publish only after the application's corresponding durable setting succeeded.
/// A superseded candidate cannot change current(); the saved setting is never changed here.
pub fn publish(candidate: PreparedLocale) -> Result<u64, PublishError> {
    CURRENT.get().ok_or(PublishError::NotInitialized)?.publish(candidate)
}
// Always compiled, though no test calls it: the application's boot calls it from code that is
// itself `cfg(not(test))`, and that code builds against this crate WITH `test-support` whenever
// cargo unifies a dependent's dev-dependency features into a non-test build of the dependent.
pub fn initialize(preference: Preference, controlled: bool) {
    set_saved_preference(preference);
    // Record/replay must not depend on the TV or process environment. The preference is part
    // of the captured session; System resolves to English and formatting is fixed in both modes.
    if controlled {
        let _ = CURRENT.set(live::Registry::new(preference, live::BootInputs::controlled())
            .expect("compiled controlled locale data"));
        return;
    }
    let info = platform_locale();
    let environment = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|s| !s.is_empty()));
    let source = if info.is_some() { "settings" }
        else if environment.is_some() { "environment" } else { "fallback" };
    let SystemLocale { ui, fmt, clock } = info.unwrap_or_default();
    #[cfg(feature = "hostsim")]
    let fmt = std::env::var("PLXNATIVE_FORMAT_LOCALE").ok().or(fmt);
    #[cfg(feature = "hostsim")]
    let forced = std::env::var("PLXNATIVE_LOCALE").ok();
    #[cfg(not(feature = "hostsim"))]
    let forced = None;
    let inputs = live::BootInputs { ui, format: fmt, clock, environment, forced,
        ..live::BootInputs::default() };
    let resolved = live::Registry::new(preference, inputs).expect("compiled locale data");
    let cx = resolved.current();
    plx_base::eventlog::log(&format!(
        "locale: source={} preference={} ui={} format={} clock={:?}",
        source, cx.preference().tag(), cx.language().tag(), cx.format_locale(), cx.clock()
    ));
    if CURRENT.set(resolved).is_err() {
        plx_base::eventlog::log("locale: initialization already completed");
    }
}
#[derive(Default)]
struct SystemLocale {
    ui: Option<String>,
    fmt: Option<String>,
    clock: Option<String>,
}
fn parse_reply(raw: &str) -> Option<SystemLocale> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    if v["returnValue"].as_bool() != Some(true) {
        return None;
    }
    let info = &v["settings"]["localeInfo"];
    if !info.is_object() {
        return None;
    }
    Some(SystemLocale {
        ui: info["locales"]["UI"].as_str().and_then(normalize),
        fmt: info["locales"]["FMT"].as_str().and_then(normalize),
        clock: info["clock"].as_str().map(str::to_string),
    })
}
fn platform_locale() -> Option<SystemLocale> {
    match crate::tv::system_locale() {
        crate::tv::LocaleReply::NoPlatform => None,
        crate::tv::LocaleReply::Reply(raw) => {
            let info = parse_reply(&raw);
            if info.is_none() {
                plx_base::eventlog::log("locale: settings refused or returned malformed localeInfo");
            }
            info
        }
        crate::tv::LocaleReply::Unavailable => {
            plx_base::eventlog::log("locale: settings unavailable; using fallback");
            None
        }
    }
}

// Every generated key has str/CStr and explicit-context variants; not every consumer needs all
// four. Keep the allowance on this generated API rather than suppressing handwritten warnings.
#[allow(dead_code)]
pub mod msg {
    include!(concat!(env!("OUT_DIR"), "/messages.rs"));
}
#[cfg(test)]
#[allow(dead_code)]
#[path = "../../build_support/catalog.rs"]
mod catalog_tests;
#[cfg(test)]
mod tests;

/// A Unix timestamp as a calendar date in the operating system's timezone. Called while building
/// cached presentation labels, never by data parsers or once per card per frame. Checked time_t conversion
/// matters on the 32-bit television: an unrepresentable date remains unknown.
pub fn local_calendar_date(seconds: i64) -> Option<[i32; 3]> {
    if seconds <= 0 { return None; }
    let stamp = libc::time_t::try_from(seconds).ok()?;
    let mut out = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: both pointers are valid for localtime_r; the result is read only on success.
    if unsafe { libc::localtime_r(&stamp, out.as_mut_ptr()) }.is_null() { return None; }
    let date = unsafe { out.assume_init() };
    Some([date.tm_year.checked_add(1900)?, date.tm_mon + 1, date.tm_mday])
}

/// Pure key for an observed, representable calendar minute. This neither reads
/// the clock nor converts timezone state, so the frame dirty gate can use it.
pub fn clock_minute_key(seconds: i64) -> Option<i64> {
    if !(1..253_402_300_800).contains(&seconds) { return None; }
    libc::time_t::try_from(seconds).ok()?;
    Some(seconds / 60)
}

/// Convert an observed Unix timestamp to local clock parts for cached UI labels.
/// Unknown/overflowing timestamps remain absent, including on 32-bit targets.
pub fn local_clock_parts(seconds: i64) -> Option<[u8; 3]> {
    if seconds <= 0 { return None; }
    let stamp = libc::time_t::try_from(seconds).ok()?;
    let mut out = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: both pointers are valid and the result is read only after success.
    if unsafe { libc::localtime_r(&stamp, out.as_mut_ptr()) }.is_null() { return None; }
    let time = unsafe { out.assume_init() };
    let parts = [u8::try_from(time.tm_hour).ok()?, u8::try_from(time.tm_min).ok()?, u8::try_from(time.tm_sec).ok()?];
    (parts[0] < 24 && parts[1] < 60 && parts[2] < 60).then_some(parts)
}

#[cfg(test)]
mod clock_label_tests {
    use super::*;
    #[test]
    fn explicit_clock_preference_and_localized_time_are_respected() {
        let twelve = LocaleContext::resolve(Preference::En, None, Some("en-US"), Some("12"), None);
        let twenty_four = LocaleContext::resolve(Preference::En, None, Some("en-US"), Some("24"), None);
        let a = twelve.time([16, 12, 20]).unwrap();
        assert!(a.starts_with("4:12:20") && a.ends_with("PM"), "{a}");
        assert_eq!(twenty_four.time([16, 12, 20]).as_deref(), Some("16:12:20"));
        assert!(twelve.time([24, 0, 0]).is_none());
        assert!(local_clock_parts(0).is_none());
    }
}

/// Civil-day bounds in the system timezone from an already observed timestamp.
/// mktime normalizes DST transitions; a day is never assumed to be 86400 seconds.
/// This is cached view preparation, not a clock read or a data parser operation.
pub fn local_day_window(seconds: i64, offset: i32) -> Option<(i64, i64)> {
    if seconds <= 0 || !(-32..=32).contains(&offset) { return None; }
    let stamp = libc::time_t::try_from(seconds).ok()?;
    let mut out = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: pointers refer to valid timestamp and output storage.
    if unsafe { libc::localtime_r(&stamp, out.as_mut_ptr()) }.is_null() { return None; }
    let mut day = unsafe { out.assume_init() };
    day.tm_hour = 0; day.tm_min = 0; day.tm_sec = 0; day.tm_isdst = -1;
    day.tm_mday = day.tm_mday.checked_add(offset)?;
    // SAFETY: mktime receives a fully initialized tm and normalizes it in place.
    let start = unsafe { libc::mktime(&mut day) };
    if start < 0 { return None; }
    day.tm_mday = day.tm_mday.checked_add(1)?;
    // The first midnight can normalize to 01:00 at a DST transition.
    // The following civil date still starts at its own midnight.
    day.tm_hour = 0; day.tm_min = 0; day.tm_sec = 0; day.tm_isdst = -1;
    let end = unsafe { libc::mktime(&mut day) };
    let (start, end) = (i64::try_from(start).ok()?, i64::try_from(end).ok()?);
    (end > start && end.checked_sub(start)? <= 172800).then_some((start, end))
}

#[cfg(test)]
mod day_window_tests {
    #[test]
    fn midnight_dst_uses_next_civil_midnight() {
        const CHILD: &str = "OTT_GUIDE_MIDNIGHT_TEST";
        if std::env::var_os(CHILD).is_some() {
            let (start, end) = super::local_day_window(1541340000, 0).unwrap();
            assert_eq!((start, end), (1541300400, 1541383200));
            assert_eq!(super::local_day_window(1541340000, 1).unwrap().0, end);
            return;
        }
        // A separate test process owns TZ; no other test sees a changed zone.
        let module = module_path!().split_once("::").unwrap().1;
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .env(CHILD, "1").env("TZ", "America/Sao_Paulo")
            .arg("--exact").arg(format!("{module}::midnight_dst_uses_next_civil_midnight"))
            .arg("--nocapture").status().unwrap();
        assert!(status.success());
    }
    #[test]
    fn civil_day_contains_sample_and_neighbor_windows_join() {
        // Includes a DST-transition date in zones which observe one. The test
        // does not mutate the process timezone or assume its specific offset.
        let stamp = 1793552400;
        let (start, end) = super::local_day_window(stamp, 0).unwrap();
        assert!(start <= stamp && stamp < end);
        assert_eq!(super::local_day_window(stamp, -1).unwrap().1, start);
        assert_eq!(super::local_day_window(stamp, 1).unwrap().0, end);
    }
    #[test]
    fn day_conversion_rejects_invalid_or_unbounded_requests() {
        assert_eq!(super::local_day_window(-1, 0), None);
        assert_eq!(super::local_day_window(i64::MAX, 0), None);
        assert_eq!(super::local_day_window(1700000000, 33), None);
    }
}

#[cfg(test)]
mod minute_clock_tests {
    use super::*;
    #[test]
    fn minute_clock_respects_12_and_24_hour_locales_without_seconds() {
        let twelve = LocaleContext::resolve(Preference::En, None, Some("en-US"), Some("12"), None);
        let full = LocaleContext::resolve(Preference::En, None, Some("en-US"), Some("24"), None);
        let a = twelve.time_minute([16, 12, 20]).unwrap();
        assert!(a.starts_with("4:12") && a.ends_with("PM"), "{a}");
        assert!(!a.contains(":20"));
        assert_eq!(full.time_minute([16, 12, 20]).as_deref(), Some("16:12"));
        assert_eq!(full.time_minute([16, 12, 59]), full.time_minute([16, 12, 0]));
        assert_eq!(full.time([16, 12, 20]).as_deref(), Some("16:12:20"));
    }
    #[test]
    fn minute_clock_rejects_invalid_parts() {
        let locale = LocaleContext::resolve(Preference::En, None, Some("en-US"), Some("24"), None);
        for parts in [[24, 0, 0], [0, 60, 0], [0, 0, 60],
            [u8::MAX, 0, 0], [0, u8::MAX, 0], [0, 0, u8::MAX]] {
            assert!(locale.time_minute(parts).is_none(), "invalid clock parts: {parts:?}");
        }
        assert_eq!(locale.time_minute([0, 0, 0]).as_deref(), Some("00:00"));
        assert_eq!(locale.time_minute([23, 59, 59]).as_deref(), Some("23:59"));
    }
}
