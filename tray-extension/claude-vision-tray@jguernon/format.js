// Mise en forme de l'état publié par Claude Vision (tray.json).
// Fonctions pures : aucune dépendance à GNOME Shell, seulement GLib,
// pour pouvoir les tester avec `gjs -m` hors du Shell.

import GLib from 'gi://GLib';

// Palette EGA, identique à l'application.
export const EGA = {
    LGREEN: '#55FF55',
    YELLOW: '#FFFF55',
    LRED: '#FF5555',
    WHITE: '#FFFFFF',
    LGRAY: '#AAAAAA',
    DGRAY: '#555555',
    BROWN: '#AA5500',
};

export const BAR_CELLS = 8;
export const FRESH_MS = 20000;

const HEX_COLOR = /^#[0-9A-Fa-f]{6}$/;

function str(v) {
    return typeof v === 'string' ? v : null;
}

function num(v) {
    return typeof v === 'number' && Number.isFinite(v) ? v : null;
}

function normAccount(a) {
    const label = str(a.label) ?? str(a.short) ?? '?';
    let error = null;
    if (a.error !== null && a.error !== undefined)
        error = typeof a.error === 'string' ? a.error : JSON.stringify(a.error);
    return {
        label,
        short: str(a.short) ?? label,
        // Couleur insérée dans un attribut Pango : on n'accepte qu'un #RRGGBB.
        color: HEX_COLOR.test(a.color ?? '') ? a.color : EGA.LGRAY,
        tier: str(a.tier),
        pct: num(a.weekly_pct),
        resetsAt: str(a.weekly_resets_at),
        fetchedAtMs: num(a.fetched_at_ms),
        error,
    };
}

/**
 * Décode le contenu de tray.json. Lève une exception si le JSON est
 * illisible ou si `updated_ms` manque (fichier partiel, format inconnu).
 *
 * @param {Uint8Array|string} data
 */
export function parseState(data) {
    const text = typeof data === 'string' ? data : new TextDecoder('utf-8').decode(data);
    const raw = JSON.parse(text);
    if (!raw || typeof raw !== 'object' || Array.isArray(raw))
        throw new Error('tray.json : objet attendu');
    const updatedMs = num(raw.updated_ms);
    if (updatedMs === null)
        throw new Error('tray.json : updated_ms manquant');
    const accounts = Array.isArray(raw.accounts)
        ? raw.accounts.filter(a => a && typeof a === 'object').map(normAccount)
        : [];
    return {
        updatedMs,
        // langue choisie dans l'app (touche l) : « fr » ou « en »
        lang: str(raw.lang) === 'fr' ? 'fr' : 'en',
        sessions: num(raw.sessions),
        busy: num(raw.busy),
        accounts,
    };
}

/** Vrai si l'état date de moins de FRESH_MS (l'app publie toutes les 5 s). */
export function isFresh(state, nowMs) {
    if (!state)
        return false;
    return Math.abs(nowMs - state.updatedMs) < FRESH_MS;
}

export function escape(text) {
    return GLib.markup_escape_text(String(text), -1);
}

export function span(text, color) {
    return `<span foreground="${color}">${escape(text)}</span>`;
}

/** Mêmes seuils que l'app : vert < 50 %, jaune < 80 %, rouge au-delà. */
export function pctColor(pct) {
    if (pct < 50)
        return EGA.LGREEN;
    if (pct < 80)
        return EGA.YELLOW;
    return EGA.LRED;
}

export function fullCells(pct, cells = BAR_CELLS) {
    const p = Math.min(100, Math.max(0, pct));
    return Math.round(p / 100 * cells);
}

export function barMarkup(pct, cells = BAR_CELLS) {
    const full = fullCells(pct, cells);
    let out = '';
    if (full > 0)
        out += span('▓'.repeat(full), pctColor(pct));
    if (cells - full > 0)
        out += span('░'.repeat(cells - full), EGA.DGRAY);
    return out;
}

function pctText(pct) {
    return `${Math.round(pct)}%`;
}

/** Ligne compacte du panneau : « Perso ▓▓▓▓▓▓░░ 73%  Work ▓░░░░░░░ 6% ». */
export function panelMarkup(state) {
    if (!state.accounts.length)
        return span('Claude —', EGA.LGRAY);
    return state.accounts.map(a => {
        let s = span(a.short, a.color);
        if (a.pct === null)
            s += ` ${span('—', EGA.LGRAY)}`;
        else
            s += ` ${barMarkup(a.pct)} ${span(pctText(a.pct), EGA.WHITE)}`;
        if (a.error !== null)
            s += ` ${span('⚠', EGA.BROWN)}`;
        return s;
    }).join('  ');
}

/**
 * Délai avant remise à zéro, même format que `fmt_reset` dans l'app :
 * « 2j 4h », « 3h 05 », « 12 min », « bientôt », « — » (en anglais : « 2d 4h », « soon »).
 *
 * @param {string|null} iso
 * @param {GLib.DateTime} [now]
 * @param {string} [lang] « fr » ou « en »
 */
export function fmtReset(iso, now = null, lang = 'en') {
    if (!iso)
        return '—';
    const dt = GLib.DateTime.new_from_iso8601(iso, null);
    if (!dt)
        return '—';
    const ref = now ?? GLib.DateTime.new_now_utc();
    const secs = Math.trunc(dt.difference(ref) / 1e6);
    if (secs <= 0)
        return lang === 'fr' ? 'bientôt' : 'soon';
    const dd = Math.floor(secs / 86400);
    const hh = Math.floor((secs % 86400) / 3600);
    const mm = Math.floor((secs % 3600) / 60);
    if (dd > 0)
        return `${dd}${lang === 'fr' ? 'j' : 'd'} ${hh}h`;
    if (hh > 0)
        return `${hh}h ${String(mm).padStart(2, '0')}`;
    return `${mm} min`;
}

/** Tronque ou complète à `n` caractères (pour aligner les colonnes du menu). */
export function fit(text, n) {
    const chars = [...String(text)];
    if (chars.length > n)
        return `${chars.slice(0, n - 1).join('')}…`;
    return chars.join('').padEnd(n);
}

function hhmm(ms) {
    const dt = GLib.DateTime.new_from_unix_local(Math.floor(ms / 1000));
    return dt ? dt.format('%H:%M') : '';
}

/** Ligne d'un compte dans le menu : libellé, forfait, barre, %, délai. */
export function accountMenuMarkup(a, now = null, lang = 'en') {
    let s = `${span(fit(a.label, 14), a.color)} ${span(fit(a.tier ?? '', 9), EGA.YELLOW)} `;
    if (a.pct === null)
        s += span(fit('—', BAR_CELLS + 5), EGA.LGRAY);
    else
        s += `${barMarkup(a.pct)} ${span(pctText(a.pct).padStart(4), EGA.WHITE)}`;
    s += `  ${span('↻', EGA.DGRAY)} ${span(fmtReset(a.resetsAt, now, lang), EGA.LGRAY)}`;
    return s;
}

/** Ligne d'erreur d'un compte (null s'il n'y en a pas). */
export function accountErrorMarkup(a, lang = 'en') {
    if (a.error === null)
        return null;
    let text = `⚠ ${a.error}`;
    if (a.fetchedAtMs !== null)
        text += ` · ${lang === 'fr' ? 'relevé' : 'as of'} ${hhmm(a.fetchedAtMs)}`;
    return span(text, EGA.BROWN);
}

/** « 5 sessions · 2 actives » (null si l'app ne publie pas ces champs). */
export function sessionsMarkup(state) {
    if (state.sessions === null)
        return null;
    const n = state.sessions;
    let text = `${n} ${n > 1 ? 'sessions' : 'session'}`;
    if (state.busy !== null) {
        const b = state.busy;
        text += ` · ${b} ${b > 1 && state.lang === 'fr' ? 'actives' : 'active'}`;
    }
    return span(text, EGA.LGRAY);
}
