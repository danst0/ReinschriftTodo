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

// Handle messages from the main thread
self.addEventListener('message', (event) => {
    if (event.data === 'skipWaiting') {
        self.skipWaiting();
    }
});

console.log('[SW] Service worker loaded');
