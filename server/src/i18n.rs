/// EN/ET localization for the server-rendered UI (roadmap item 4).
/// One flat struct of strings per language; templates only ever see `t.*`.
/// Language resolves from the `lang` query param, then the `lang` cookie,
/// then English.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Lang {
    En,
    Et,
}

impl Lang {
    pub fn from_code(code: &str) -> Option<Lang> {
        match code {
            "en" => Some(Lang::En),
            "et" => Some(Lang::Et),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Et => "et",
        }
    }

    pub fn verdict(self, fav: f64) -> (&'static str, &'static str) {
        let class = if fav >= 0.6 { "win" } else if fav >= 0.45 { "mixed" } else { "flop" };
        let word = match (self, class) {
            (Lang::En, "win") => "LIKELY WINS",
            (Lang::En, "mixed") => "MIXED",
            (Lang::En, _) => "LIKELY FLOPS",
            (Lang::Et, "win") => "TÕENÄOLINE VÕIT",
            (Lang::Et, "mixed") => "VASTUOLULINE",
            (Lang::Et, _) => "TÕENÄOLINE LÄBIKUKKUMINE",
        };
        (word, class)
    }
}

/// All user-visible strings. Field names match what templates reference.
pub struct T {
    pub lang: &'static str,
    pub other_lang: &'static str,
    pub toggle_label: &'static str,
    pub tagline: &'static str,

    pub submit_heading: &'static str,
    pub draft_placeholder: &'static str,
    pub run_button: &'static str,
    pub engine_label: &'static str,
    pub engine_model: &'static str,
    pub engine_mock: &'static str,

    pub runs_heading: &'static str,
    pub loading: &'static str,
    pub no_runs: &'static str,
    pub pending: &'static str,
    pub replies_word: &'static str,
    pub archive_word: &'static str,
    pub unarchive_word: &'static str,
    pub archived_shelf: &'static str,
    pub back_to_active: &'static str,
    pub no_archived: &'static str,
    pub weighted_fav_short: &'static str,

    pub back_all_runs: &'static str,
    pub run_word: &'static str,
    pub panel_prediction: &'static str,
    pub weighted_fav: &'static str,
    pub unweighted: &'static str,
    pub weighting_moved: &'static str,
    pub responders: &'static str,
    pub sentiment: &'static str,
    pub positive: &'static str,
    pub neutral: &'static str,
    pub negative: &'static str,
    pub by_archetype: &'static str,
    pub by_locality: &'static str,
    pub archetype_col: &'static str,
    pub locality_col: &'static str,
    pub weight_col: &'static str,
    pub fav_col: &'static str,
    pub raw_json: &'static str,
    pub no_prediction_yet: &'static str,
    pub replies_so_far: &'static str,
    pub refreshes_hint: &'static str,
    pub engine_word: &'static str,

    pub persona_reactions: &'static str,
    pub locked_notice: &'static str,
    pub lock: &'static str,
    pub unlock: &'static str,

    pub notes_heading: &'static str,
    pub notes_hint: &'static str,
    pub notes_placeholder: &'static str,
    pub save_notes: &'static str,
    pub saved: &'static str,

    pub outcome_heading: &'static str,
    pub outcome_hint: &'static str,
    pub success_label: &'static str,
    pub ctr_label: &'static str,
    pub conversion_label: &'static str,
    pub engagement_label: &'static str,
    pub record_outcome: &'static str,
    pub predicted_word: &'static str,
    pub actual_word: &'static str,
    pub error_word: &'static str,

    pub calibration_heading: &'static str,
    pub calibration_hint: &'static str,
    pub calibration_empty: &'static str,
    pub mean_error: &'static str,
    pub item_col: &'static str,
}

pub fn t(lang: Lang) -> &'static T {
    match lang {
        Lang::En => &EN,
        Lang::Et => &ET,
    }
}

pub static EN: T = T {
    lang: "en",
    other_lang: "et",
    toggle_label: "eesti keeles",
    tagline: "rust + htmx on the rapids · prototype backend",

    submit_heading: "Submit a draft to the panel",
    draft_placeholder: "Paste the draft copy / public communication to stress-test…",
    run_button: "Run the panel",
    engine_label: "Reply engine",
    engine_model: "model (real LLM personas)",
    engine_mock: "mock (deterministic, free)",

    runs_heading: "Runs",
    loading: "Loading…",
    no_runs: "No runs yet. Submit a draft above — anything that lands on the rapids shows up here.",
    pending: "PENDING",
    replies_word: "replies",
    archive_word: "archive",
    unarchive_word: "restore",
    archived_shelf: "archived",
    back_to_active: "← active runs",
    no_archived: "Nothing archived. The archive button on a run card moves it here, out of the main list.",
    weighted_fav_short: "weighted favorability",

    back_all_runs: "← all runs",
    run_word: "Run",
    panel_prediction: "Panel prediction",
    weighted_fav: "Weighted favorability",
    unweighted: "unweighted",
    weighting_moved: "weighting moved it",
    responders: "responders",
    sentiment: "Sentiment:",
    positive: "positive",
    neutral: "neutral",
    negative: "negative",
    by_archetype: "By archetype",
    by_locality: "By locality",
    archetype_col: "archetype",
    locality_col: "locality",
    weight_col: "weight",
    fav_col: "favorability",
    raw_json: "raw prediction JSON (verbatim from the rapids)",
    no_prediction_yet: "No prediction yet —",
    replies_so_far: "replies so far.",
    refreshes_hint: "This view refreshes itself until the prediction lands on the queue.",
    engine_word: "engine",

    persona_reactions: "Persona reactions",
    locked_notice: "🔒 Locked voice — reaction text withheld by policy. Its score still contributes to the prediction; the text never leaves the panel.",
    lock: "lock",
    unlock: "unlock",

    notes_heading: "Operator notes",
    notes_hint: "(held only in this backend — never on the rapids)",
    notes_placeholder: "Context, decisions, follow-ups — enrichment stored only in this project.",
    save_notes: "Save notes",
    saved: "saved ✓",

    outcome_heading: "Real-world outcome",
    outcome_hint: "Close the calibration loop: record how this actually performed. Appended to the rapids as outcome:<item> in OVK's format.",
    success_label: "success (0–1, required)",
    ctr_label: "CTR (0–1)",
    conversion_label: "conversion (0–1)",
    engagement_label: "engagement (≥ 0)",
    record_outcome: "Record outcome",
    predicted_word: "predicted",
    actual_word: "actual",
    error_word: "signed error",

    calibration_heading: "Calibration",
    calibration_hint: "Predictions joined to their recorded real outcomes. Directional-only until enough cycles accrue.",
    calibration_empty: "No calibrated runs yet — record an outcome on a predicted run to start the loop.",
    mean_error: "mean signed error",
    item_col: "run",
};

pub static ET: T = T {
    lang: "et",
    other_lang: "en",
    toggle_label: "in English",
    tagline: "rust + htmx · prototüübi taustsüsteem",

    submit_heading: "Esita mustand paneelile",
    draft_placeholder: "Kleebi siia avalik sõnum / mustand, mida testida…",
    run_button: "Käivita paneel",
    engine_label: "Vastuste mootor",
    engine_model: "mudel (päris LLM-personad)",
    engine_mock: "mock (deterministlik, tasuta)",

    runs_heading: "Käivitused",
    loading: "Laadimine…",
    no_runs: "Käivitusi veel pole. Esita mustand ülal — kõik, mis jõuab järjekorda, ilmub siia.",
    pending: "OOTEL",
    replies_word: "vastust",
    archive_word: "arhiveeri",
    unarchive_word: "taasta",
    archived_shelf: "arhiveeritud",
    back_to_active: "← aktiivsed käivitused",
    no_archived: "Arhiiv on tühi. Käivituse kaardil olev arhiveerimisnupp tõstab selle siia, põhinimekirjast välja.",
    weighted_fav_short: "kaalutud soosing",

    back_all_runs: "← kõik käivitused",
    run_word: "Käivitus",
    panel_prediction: "Paneeli ennustus",
    weighted_fav: "Kaalutud soosing",
    unweighted: "kaalumata",
    weighting_moved: "kaalumine nihutas",
    responders: "vastajat",
    sentiment: "Meelsus:",
    positive: "positiivne",
    neutral: "neutraalne",
    negative: "negatiivne",
    by_archetype: "Arhetüübi järgi",
    by_locality: "Piirkonna järgi",
    archetype_col: "arhetüüp",
    locality_col: "piirkond",
    weight_col: "kaal",
    fav_col: "soosing",
    raw_json: "ennustuse toores JSON (muutmata kujul järjekorrast)",
    no_prediction_yet: "Ennustust veel pole —",
    replies_so_far: "vastust seni.",
    refreshes_hint: "See vaade värskendab end, kuni ennustus järjekorda jõuab.",
    engine_word: "mootor",

    persona_reactions: "Personade reaktsioonid",
    locked_notice: "🔒 Lukustatud hääl — reaktsiooni tekst on poliitika järgi varjatud. Skoor läheb ennustusse; tekst paneelist ei välju.",
    lock: "lukusta",
    unlock: "ava",

    notes_heading: "Operaatori märkmed",
    notes_hint: "(hoitakse ainult siin taustsüsteemis — mitte kunagi järjekorras)",
    notes_placeholder: "Kontekst, otsused, järeltegevused — rikastus, mis elab ainult selles projektis.",
    save_notes: "Salvesta märkmed",
    saved: "salvestatud ✓",

    outcome_heading: "Tegelik tulemus",
    outcome_hint: "Sulge kalibreerimisahel: salvesta, kuidas see päriselt toimis. Lisatakse järjekorda kujul outcome:<item> OVK vormingus.",
    success_label: "edukus (0–1, kohustuslik)",
    ctr_label: "CTR (0–1)",
    conversion_label: "konversioon (0–1)",
    engagement_label: "kaasatus (≥ 0)",
    record_outcome: "Salvesta tulemus",
    predicted_word: "ennustatud",
    actual_word: "tegelik",
    error_word: "märgiga viga",

    calibration_heading: "Kalibreerimine",
    calibration_hint: "Ennustused ühendatuna tegelike tulemustega. Suunda näitav, kuni tsükleid koguneb piisavalt.",
    calibration_empty: "Kalibreeritud käivitusi veel pole — salvesta ennustatud käivitusele tulemus.",
    mean_error: "keskmine märgiga viga",
    item_col: "käivitus",
};
