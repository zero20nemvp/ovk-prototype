use std::path::Path;

use crate::store::PersonaSeed;

/// Persona mapping (roadmap item 3): OVK's marketing archetypes cast as the
/// red-team panel of the prototype. Identity (id, archetype, display name)
/// comes read-only from OVK's panel files; the ROLE each archetype plays on a
/// communications red-team — and which voices are locked by default — is this
/// project's enrichment, held only here.
///
/// The hostile-amplification voice (Outlaw) ships locked: its score counts,
/// its text is never shown. Operators can unlock per persona; that choice
/// sticks (lock_touched) across reseeds.
fn role_for(archetype: &str) -> (&'static str, &'static str, bool) {
    match archetype {
        "Everyman" => ("Ordinary reader", "Tavalugeja", false),
        "Caregiver" => ("Concerned parent", "Murelik lapsevanem", false),
        "Explorer" => ("Early adopter", "Varajane omaksvõtja", false),
        "Creator" => ("Creative professional", "Loovprofessionaal", false),
        "Sage" => ("Investigative journalist", "Uuriv ajakirjanik", false),
        "Ruler" => ("Regulator / official", "Ametnik", false),
        "Lover" => ("Sympathetic reader", "Poolehoidja", false),
        "Innocent" => ("Trusting reader", "Usaldav lugeja", false),
        "Jester" => ("Meme amplifier", "Meemivõimendaja", false),
        "Outlaw" => ("Hostile amplifier", "Vaenulik võimendaja", true),
        "Hero" => ("Watchdog", "Valvekoer", false),
        "Magician" => ("Spin analyst", "Sõnumianalüütik", false),
        _ => ("Panel voice", "Paneelihääl", false),
    }
}

pub fn load(ovk_root: &Path) -> Vec<PersonaSeed> {
    let Ok(weights) = std::fs::read_to_string(ovk_root.join("panel/weights.tsv")) else {
        return Vec::new();
    };
    weights
        .lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            let (id, _w, arch) = (f.next()?, f.next()?, f.next()?);
            if id.is_empty() {
                return None;
            }
            let (role, role_et, locked_default) = role_for(arch);
            Some(PersonaSeed {
                id: id.to_string(),
                display_name: display_name(ovk_root, id),
                archetype: arch.to_string(),
                role: role.to_string(),
                role_et: role_et.to_string(),
                locked_default,
            })
        })
        .collect()
}

/// Full name from the persona file's title line, e.g.
/// `# Panel Persona — Aisha Okafor ("Creator")` → "Aisha Okafor".
fn display_name(ovk_root: &Path, id: &str) -> String {
    let fallback = || {
        let mut c = id.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    };
    let Ok(md) = std::fs::read_to_string(ovk_root.join(format!("panel/personas/{id}.md"))) else {
        return fallback();
    };
    let Some(title) = md.lines().next() else { return fallback() };
    let after = match title.split_once("— ") {
        Some((_, rest)) => rest,
        None => return fallback(),
    };
    let name = after.split(" (\"").next().unwrap_or(after).trim();
    if name.is_empty() { fallback() } else { name.to_string() }
}
