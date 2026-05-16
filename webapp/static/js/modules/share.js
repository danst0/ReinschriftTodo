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
            <div class="share-dialog-link-row hidden">
                <input type="text" class="share-dialog-link" readonly>
                <button type="button" class="btn-secondary share-dialog-copy"></button>
            </div>
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

function setState(state) {
    const el = ensureDialog();
    el.querySelector('.share-dialog-title').textContent =
        t('share', 'Teilen') + ': +' + state.project;
    el.querySelector('.share-dialog-hint').textContent =
        t('shareHint', 'Mit diesem Link können andere die Aufgaben dieses Projekts sehen, abhaken und neue hinzufügen — ohne Anmeldung.');

    const linkRow = el.querySelector('.share-dialog-link-row');
    const linkInput = el.querySelector('.share-dialog-link');
    const copyBtn = el.querySelector('.share-dialog-copy');
    const revokeBtn = el.querySelector('.share-dialog-revoke');
    const createBtn = el.querySelector('.share-dialog-create');
    const closeBtn = el.querySelector('.share-dialog-close');

    copyBtn.textContent = t('shareCopy', 'Link kopieren');
    revokeBtn.textContent = t('shareRevoke', 'Link entfernen');
    createBtn.textContent = t('shareCreate', 'Link erstellen');
    closeBtn.textContent = t('close', 'Schließen');

    if (state.url) {
        linkRow.classList.remove('hidden');
        linkInput.value = state.url;
        revokeBtn.classList.remove('hidden');
        createBtn.classList.add('hidden');
    } else {
        linkRow.classList.add('hidden');
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
        const resp = await fetchWithCsrf(`/api/shares/${encodeURIComponent(state.project)}`, {
            method: 'POST'
        });
        if (resp.ok) {
            const data = await resp.json();
            setState({ project: state.project, url: data.url });
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
            setState({ project, url: data.url });
        }
    } catch (e) {
        // ignore — dialog already showing "Erstellen" state
    }
}
