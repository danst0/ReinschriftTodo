/**
 * Web Push reminders: the settings toggle that subscribes this device.
 *
 * The server decides when to remind (app/services/push_service.py) and the
 * service worker shows the notification, so reminders arrive with the app
 * closed. This module only manages the subscription.
 */

import { postJson } from './api.js';

const SW_URL = '/static/sw.js';

let t = {};
let onTodosChanged = null;

/**
 * Wire up the reminder controls in the settings dialog.
 * @param {object} options
 * @param {object} options.translations - push* strings from the template
 * @param {Function} options.onTodosChanged - called after a notification action changed a todo
 */
export async function initPush(options = {}) {
    t = options.translations || {};
    onTodosChanged = options.onTodosChanged || null;

    const toggle = document.getElementById('push-toggle');
    const testBtn = document.getElementById('push-test');
    if (!toggle) return;

    if (!('serviceWorker' in navigator) || !('PushManager' in window) || !('Notification' in window)) {
        toggle.disabled = true;
        // iOS only offers push to web apps installed on the home screen.
        const isIos = /iPad|iPhone|iPod/.test(navigator.userAgent)
            || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
        setStatus(isIos ? t.pushIosHint : t.pushUnsupported);
        return;
    }

    // A done/postpone tapped in a notification changes the file behind the page.
    navigator.serviceWorker.addEventListener('message', (event) => {
        if (event.data && event.data.type === 'todos-changed' && onTodosChanged) {
            onTodosChanged();
        }
    });

    toggle.addEventListener('change', () => {
        (toggle.checked ? enable() : disable()).catch((err) => {
            console.warn('Push setup failed:', err);
            setStatus(t.pushError);
        }).finally(refresh);
    });
    if (testBtn) {
        testBtn.addEventListener('click', sendTest);
    }

    await refresh();

    // Re-announce an existing subscription: the server may have dropped it
    // (expired, config volume reset), and the language may have changed.
    const subscription = await currentSubscription();
    if (subscription && Notification.permission === 'granted') {
        postJson('/api/push/subscribe', subscription.toJSON()).catch(() => {});
    }
}

async function registration() {
    // register() returns the existing registration when there is one. The
    // worker's scope is /static/, so navigator.serviceWorker.ready would never
    // resolve for this page — push does not need the page to be controlled.
    return navigator.serviceWorker.register(SW_URL);
}

async function currentSubscription() {
    const reg = await registration();
    return reg.pushManager.getSubscription();
}

async function enable() {
    // Must run inside the click: browsers ignore permission prompts that no
    // user gesture asked for.
    const permission = await Notification.requestPermission();
    if (permission !== 'granted') {
        setStatus(t.pushDenied);
        return;
    }

    const keyResponse = await fetch('/api/push/public-key', {
        headers: { 'X-Requested-With': 'XMLHttpRequest' }
    });
    if (!keyResponse.ok) throw new Error(`public key: ${keyResponse.status}`);
    const { publicKey } = await keyResponse.json();

    const reg = await registration();
    let subscription = await reg.pushManager.getSubscription();
    if (subscription && !sameKey(subscription, publicKey)) {
        // Subscribed against an older server key: the server can no longer
        // reach it.
        await subscription.unsubscribe();
        subscription = null;
    }
    if (!subscription) {
        subscription = await reg.pushManager.subscribe({
            userVisibleOnly: true,
            applicationServerKey: urlBase64ToUint8Array(publicKey)
        });
    }

    const response = await postJson('/api/push/subscribe', subscription.toJSON());
    if (!response.ok) throw new Error(`subscribe: ${response.status}`);
    setStatus('');
}

async function disable() {
    const subscription = await currentSubscription();
    if (!subscription) return;
    await postJson('/api/push/unsubscribe', { endpoint: subscription.endpoint }).catch(() => {});
    await subscription.unsubscribe();
    setStatus('');
}

async function sendTest() {
    const subscription = await currentSubscription();
    if (!subscription) return;
    try {
        const response = await postJson('/api/push/test', { endpoint: subscription.endpoint });
        setStatus(response.ok ? t.pushTestSent : t.pushError);
    } catch (err) {
        setStatus(t.pushError);
    }
    await refresh();
}

async function refresh() {
    const toggle = document.getElementById('push-toggle');
    const testBtn = document.getElementById('push-test');
    const subscribed = Boolean(await currentSubscription()) && Notification.permission === 'granted';
    toggle.checked = subscribed;
    if (testBtn) testBtn.hidden = !subscribed;
    if (Notification.permission === 'denied') {
        toggle.disabled = true;
        setStatus(t.pushDenied);
    }
}

function setStatus(text) {
    const status = document.getElementById('push-status');
    if (!status) return;
    status.textContent = text || '';
    status.hidden = !text;
}

function sameKey(subscription, publicKey) {
    const current = subscription.options && subscription.options.applicationServerKey;
    if (!current) return true;  // Browser does not tell; assume it matches.
    const expected = urlBase64ToUint8Array(publicKey);
    const actual = new Uint8Array(current);
    return actual.length === expected.length && actual.every((byte, i) => byte === expected[i]);
}

function urlBase64ToUint8Array(base64) {
    const padded = (base64 + '='.repeat((4 - base64.length % 4) % 4))
        .replace(/-/g, '+').replace(/_/g, '/');
    return Uint8Array.from(atob(padded), (c) => c.charCodeAt(0));
}
