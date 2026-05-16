/**
 * Share dialog: manage public share-link for a project.
 *
 * Backed by:
 *   - GET    /api/shares/<project>  → existing share or null
 *   - POST   /api/shares/<project>  → create (idempotent)
 *   - DELETE /api/shares/<project>  → revoke
 */

import { fetchWithCsrf } from './api.js';

let dialogEl = null;
let translations = {};

const TTL_OPTIONS = [
    { value: 7, labelKey: 'shareValidity7', fallback: '7 Tage' },
    { value: 30, labelKey: 'shareValidity30', fallback: '30 Tage' },
    { value: 90, labelKey: 'shareValidity90', fallback: '90 Tage' },
    { value: null, labelKey: 'shareValidityForever', fallback: 'Unbefristet' },
];
const DEFAULT_TTL = 30;

export function configureShare(t = {}) {
    translations = t;
}

function ensureDialog() {
    if (dialogEl) return dialogEl;

    dialogEl = document.createElement('div');
    dialogEl.id = 'shareDialog';
    dialogEl.className = 'share-dialog hidden';
    dialogEl.innerHTML = `
        <div class="share-dialog-backdrop" data-close="1"></div>
        <div class="share-dialog-box" role="dialog" aria-modal="true">
            <h3 class="share-dialog-title"></h3>
            <p class="share-dialog-hint"></p>
            <div class="share-dialog-ttl-row hidden">
                <label class="share-dialog-ttl-label" for="shareDialogTtl"></label>
                <select id="shareDialogTtl" class="share-dialog-ttl"></select>
            </div>
            <div class="share-dialog-link-row hidden">
                <input type="text" class="share-dialog-link" readonly>
                <button type="button" class="btn-secondary share-dialog-copy"></button>
            </div>
            <p class="share-dialog-expiry hidden"></p>
            <div class="share-dialog-actions">
                <button type="button" class="btn-secondary share-dialog-revoke hidden"></button>
                <button type="button" class="btn-primary share-dialog-create hidden"></button>
                <button type="button" class="btn-secondary share-dialog-close"></button>
            </div>
        </div>
    `;
    document.body.appendChild(dialogEl);

    dialogEl.addEventListener('click', (ev) => {
        if (ev.target.dataset.close === '1' || ev.target.classList.contains('share-dialog-close')) {
            closeDialog();
        }
    });
    document.addEventListener('keydown', (ev) => {
        if (ev.key === 'Escape' && !dialogEl.classList.contains('hidden')) {
            closeDialog();
        }
    });

    return dialogEl;
}

function closeDialog() {
    if (dialogEl) dialogEl.classList.add('hidden');
}

function t(key, fallback) {
    return (translations && translations[key]) || fallback;
}

function populateTtlSelect(selectEl) {
    // Build options once; preserves user's choice across re-renders of the
    // same dialog instance because we keep the element around.
    if (selectEl.options.length) return;
    for (const opt of TTL_OPTIONS) {
        const option = document.createElement('option');
        // Use empty string for the "never expires" case — value is round-tripped
        // back to null when read.
        option.value = opt.value === null ? '' : String(opt.value);
        option.textContent = t(opt.labelKey, opt.fallback);
        if (opt.value === DEFAULT_TTL) option.selected = true;
        selectEl.appendChild(option);
    }
}

function formatExpiry(isoString) {
    if (!isoString) return null;
    const d = new Date(isoString);
    if (Number.isNaN(d.getTime())) return null;
    try {
        return new Intl.DateTimeFormat(undefined, { dateStyle: 'long' }).format(d);
    } catch (e) {
        return d.toLocaleDateString();
    }
}

function setState(state) {
    const el = ensureDialog();
    el.querySelector('.share-dialog-title').textContent =
        t('share', 'Teilen') + ': +' + state.project;
    el.querySelector('.share-dialog-hint').textContent =
        t('shareHint', 'Mit diesem Link können andere die Aufgaben dieses Projekts sehen, abhaken und neue hinzufügen — ohne Anmeldung.');

    const ttlRow = el.querySelector('.share-dialog-ttl-row');
    const ttlLabel = el.querySelector('.share-dialog-ttl-label');
    const ttlSelect = el.querySelector('.share-dialog-ttl');
    const linkRow = el.querySelector('.share-dialog-link-row');
    const linkInput = el.querySelector('.share-dialog-link');
    const copyBtn = el.querySelector('.share-dialog-copy');
    const expiryLine = el.querySelector('.share-dialog-expiry');
    const revokeBtn = el.querySelector('.share-dialog-revoke');
    const createBtn = el.querySelector('.share-dialog-create');
    const closeBtn = el.querySelector('.share-dialog-close');

    ttlLabel.textContent = t('shareValidity', 'Gültigkeit');
    populateTtlSelect(ttlSelect);
    copyBtn.textContent = t('shareCopy', 'Link kopieren');
    revokeBtn.textContent = t('shareRevoke', 'Link entfernen');
    createBtn.textContent = t('shareCreate', 'Link erstellen');
    closeBtn.textContent = t('close', 'Schließen');

    if (state.url) {
        ttlRow.classList.add('hidden');
        linkRow.classList.remove('hidden');
        linkInput.value = state.url;
        revokeBtn.classList.remove('hidden');
        createBtn.classList.add('hidden');

        const formatted = formatExpiry(state.expires_at);
        if (formatted) {
            const tmpl = t('shareValidUntil', 'Gültig bis {date}');
            expiryLine.textContent = tmpl.replace('{date}', formatted);
        } else {
            expiryLine.textContent = t('shareValidForever', 'Unbefristet gültig');
        }
        expiryLine.classList.remove('hidden');
    } else {
        ttlRow.classList.remove('hidden');
        linkRow.classList.add('hidden');
        expiryLine.classList.add('hidden');
        revokeBtn.classList.add('hidden');
        createBtn.classList.remove('hidden');
    }

    copyBtn.onclick = async () => {
        try {
            await navigator.clipboard.writeText(linkInput.value);
            const orig = copyBtn.textContent;
            copyBtn.textContent = t('shareCopied', 'Kopiert!');
            setTimeout(() => { copyBtn.textContent = orig; }, 1200);
        } catch (e) {
            linkInput.select();
        }
    };

    revokeBtn.onclick = async () => {
        const msg = t('shareRevokeConfirm', 'Diesen Link wirklich entfernen?');
        if (!window.confirm(msg)) return;
        const resp = await fetchWithCsrf(`/api/shares/${encodeURIComponent(state.project)}`, {
            method: 'DELETE'
        });
        if (resp.ok) {
            setState({ project: state.project, url: null });
        }
    };

    createBtn.onclick = async () => {
        const raw = ttlSelect.value;
        const ttlDays = raw === '' ? null : parseInt(raw, 10);
        const resp = await fetchWithCsrf(`/api/shares/${encodeURIComponent(state.project)}`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ ttl_days: ttlDays }),
        });
        if (resp.ok) {
            const data = await resp.json();
            setState({ project: state.project, url: data.url, expires_at: data.expires_at });
        }
    };

    el.classList.remove('hidden');
}

export async function openShareDialog(ev, project) {
    if (ev && ev.preventDefault) ev.preventDefault();
    if (!project) return;
    ensureDialog();
    setState({ project, url: null });
    try {
        const resp = await fetch(`/api/shares/${encodeURIComponent(project)}`, {
            headers: { 'Accept': 'application/json' }
        });
        if (resp.ok) {
            const data = await resp.json();
            setState({ project, url: data.url, expires_at: data.expires_at });
        }
    } catch (e) {
        // ignore — dialog already showing "Erstellen" state
    }
}
