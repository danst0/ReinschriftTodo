// Reinschrift Service Worker
// Provides offline viewing capability for the todo list

const CACHE_NAME = 'reinschrift-v1';
const OFFLINE_URL = '/offline.html';

// Static assets to cache on install
const STATIC_ASSETS = [
    '/static/styles.css',
    '/static/favicon.png',
    '/static/manifest.json',
    '/static/icons/icon-192.png',
    '/static/icons/icon-512.png'
];

// Install event - cache static assets
self.addEventListener('install', (event) => {
    console.log('[SW] Installing service worker...');
    
    event.waitUntil(
        caches.open(CACHE_NAME)
            .then((cache) => {
                console.log('[SW] Caching static assets');
                return cache.addAll(STATIC_ASSETS);
            })
            .then(() => {
                // Skip waiting to activate immediately
                return self.skipWaiting();
            })
    );
});

// Activate event - clean up old caches
self.addEventListener('activate', (event) => {
    console.log('[SW] Activating service worker...');
    
    event.waitUntil(
        caches.keys().then((cacheNames) => {
            return Promise.all(
                cacheNames
                    .filter((name) => name !== CACHE_NAME)
                    .map((name) => {
                        console.log('[SW] Deleting old cache:', name);
                        return caches.delete(name);
                    })
            );
        }).then(() => {
            // Take control of all clients immediately
            return self.clients.claim();
        })
    );
});

// Fetch event - network first with cache fallback for app shell
self.addEventListener('fetch', (event) => {
    const url = new URL(event.request.url);
    
    // Skip non-GET requests
    if (event.request.method !== 'GET') {
        return;
    }
    
    // Skip external requests
    if (url.origin !== location.origin) {
        return;
    }
    
    // Skip API requests - always go to network
    if (url.pathname.startsWith('/api/') || 
        url.pathname.startsWith('/toggle/') ||
        url.pathname.startsWith('/postpone/') ||
        url.pathname.startsWith('/edit/') ||
        url.pathname.startsWith('/delete/') ||
        url.pathname.startsWith('/add')) {
        return;
    }
    
    // Static assets - cache first
    if (url.pathname.startsWith('/static/')) {
        event.respondWith(
            caches.match(event.request)
                .then((cached) => {
                    if (cached) {
                        // Return cached, but also update cache in background
                        fetch(event.request)
                            .then((response) => {
                                if (response.ok) {
                                    caches.open(CACHE_NAME)
                                        .then((cache) => cache.put(event.request, response));
                                }
                            })
                            .catch(() => {});
                        return cached;
                    }
                    
                    // Not cached, fetch from network
                    return fetch(event.request)
                        .then((response) => {
                            if (response.ok) {
                                const clone = response.clone();
                                caches.open(CACHE_NAME)
                                    .then((cache) => cache.put(event.request, clone));
                            }
                            return response;
                        });
                })
        );
        return;
    }
    
    // App shell (main page) - network first with cache fallback
    if (url.pathname === '/' || url.pathname === '/index') {
        event.respondWith(
            fetch(event.request)
                .then((response) => {
                    if (response.ok) {
                        // Cache the latest version of the main page
                        const clone = response.clone();
                        caches.open(CACHE_NAME)
                            .then((cache) => {
                                // Store with a normalized key
                                cache.put('/', clone);
                            });
                    }
                    return response;
                })
                .catch(() => {
                    // Network failed, try to serve cached version
                    console.log('[SW] Network failed, serving cached app shell');
                    return caches.match('/');
                })
        );
        return;
    }
    
    // Other pages - network first, no cache
    event.respondWith(
        fetch(event.request)
            .catch(() => {
                // Could return an offline page here if needed
                return new Response('Offline - please check your connection', {
                    status: 503,
                    statusText: 'Service Unavailable',
                    headers: new Headers({
                        'Content-Type': 'text/plain'
                    })
                });
            })
    );
});

// ---------------------------------------------------------------------------
// Push reminders (see app/services/push_service.py)
// ---------------------------------------------------------------------------

self.addEventListener('push', (event) => {
    let data = {};
    try {
        data = event.data ? event.data.json() : {};
    } catch (err) {
        data = { title: 'Reinschrift', body: event.data ? event.data.text() : '' };
    }

    event.waitUntil(self.registration.showNotification(data.title || 'Reinschrift', {
        body: data.body || '',
        tag: data.tag,
        icon: '/static/icons/icon-192.png',
        actions: data.actions || [],
        data: { marker: data.marker || null }
    }));
});

// Complete or postpone right from the notification. The request carries the
// session cookie like any page request; if it fails (logged out, todo gone),
// open the app so the user can see what happened.
async function runNotificationAction(action, marker) {
    const endpoint = action === 'done' ? '/api/toggle-batch' : '/api/postpone-batch';
    const body = { line_indexes: [-1], markers: [marker] };
    if (action === 'done') {
        body.done = true;
    } else {
        body.target = 'tomorrow';
    }
    try {
        const response = await fetch(endpoint, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body)
        });
        const result = response.ok ? await response.json() : null;
        return Boolean(result && result.updated > 0);
    } catch (err) {
        return false;
    }
}

async function focusOrOpenApp() {
    const windows = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
    for (const client of windows) {
        if (new URL(client.url).origin === location.origin && 'focus' in client) {
            return client.focus();
        }
    }
    return self.clients.openWindow('/');
}

async function notifyPagesChanged() {
    const windows = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
    windows.forEach((client) => client.postMessage({ type: 'todos-changed' }));
}

self.addEventListener('notificationclick', (event) => {
    const notification = event.notification;
    const marker = notification.data && notification.data.marker;
    notification.close();

    if ((event.action === 'done' || event.action === 'tomorrow') && marker) {
        event.waitUntil(runNotificationAction(event.action, marker).then((ok) => (
            ok ? notifyPagesChanged() : focusOrOpenApp()
        )));
        return;
    }
    event.waitUntil(focusOrOpenApp());
});

// The push service rotated the subscription: tell the server about the new
// one, or reminders stop arriving without anyone noticing.
self.addEventListener('pushsubscriptionchange', (event) => {
    event.waitUntil((async () => {
        const oldEndpoint = event.oldSubscription && event.oldSubscription.endpoint;
        let subscription = event.newSubscription;
        if (!subscription) {
            const keyResponse = await fetch('/api/push/public-key');
            if (!keyResponse.ok) return;
            const { publicKey } = await keyResponse.json();
            subscription = await self.registration.pushManager.subscribe({
                userVisibleOnly: true,
                applicationServerKey: urlBase64ToUint8Array(publicKey)
            });
        }
        const tokenResponse = await fetch('/api/csrf-token', {
            headers: { 'X-Requested-With': 'XMLHttpRequest' }
        });
        if (!tokenResponse.ok) return;
        const { csrf_token: csrfToken } = await tokenResponse.json();
        const headers = { 'Content-Type': 'application/json', 'X-CSRFToken': csrfToken };
        await fetch('/api/push/subscribe', {
            method: 'POST', headers, body: JSON.stringify(subscription.toJSON())
        });
        if (oldEndpoint && oldEndpoint !== subscription.endpoint) {
            await fetch('/api/push/unsubscribe', {
                method: 'POST', headers, body: JSON.stringify({ endpoint: oldEndpoint })
            });
        }
    })().catch((err) => console.log('[SW] Resubscribe failed:', err)));
});

function urlBase64ToUint8Array(base64) {
    const padded = (base64 + '='.repeat((4 - base64.length % 4) % 4))
        .replace(/-/g, '+').replace(/_/g, '/');
    return Uint8Array.from(atob(padded), (c) => c.charCodeAt(0));
}

// Handle messages from the main thread
self.addEventListener('message', (event) => {
    if (event.data === 'skipWaiting') {
        self.skipWaiting();
    }
});

console.log('[SW] Service worker loaded');
