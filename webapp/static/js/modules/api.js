/**
 * API utilities with CSRF handling.
 */

// Global CSRF token - will be set from main app
let csrfToken = '';

/**
 * Set the CSRF token for all API calls.
 * @param {string} token - CSRF token from server
 */
export function setCsrfToken(token) {
    csrfToken = token;
}

/**
 * Get the current CSRF token.
 * @returns {string}
 */
export function getCsrfToken() {
    return csrfToken;
}

/**
 * Fetch a fresh CSRF token and adopt it.
 * @returns {Promise<boolean>} Whether a new token was obtained
 */
async function refreshCsrfToken() {
    try {
        const res = await fetch('/api/csrf-token', {
            headers: { 'X-Requested-With': 'XMLHttpRequest' }
        });
        if (!res.ok) return false;
        const data = await res.json();
        if (!data || !data.csrf_token) return false;
        csrfToken = data.csrf_token;
        return true;
    } catch (err) {
        return false;
    }
}

/**
 * Whether a response is the server saying "your token is stale, ask again".
 * Anything else that returns 400 is a real rejection and must not be repeated.
 * @param {Response} response
 * @returns {Promise<boolean>}
 */
async function isExpiredCsrf(response) {
    if (response.status !== 400) return false;
    try {
        const data = await response.clone().json();
        return Boolean(data) && data.error === 'csrf_expired';
    } catch (err) {
        return false;
    }
}

/**
 * Make a fetch request with CSRF token.
 *
 * The token is baked into the page at render time and the partial reloads
 * never replace it, so a page open long enough carries a token the server no
 * longer accepts. Retrying once with a fresh one is safe: CSRFProtect rejects
 * before the view runs, so the first attempt provably wrote nothing.
 *
 * @param {string} url - Request URL
 * @param {object} options - Fetch options
 * @returns {Promise<Response>}
 */
export async function fetchWithCsrf(url, options = {}) {
    const send = () => fetch(url, {
        ...options,
        headers: { ...(options.headers || {}), 'X-CSRFToken': csrfToken }
    });

    const response = await send();

    if (!await isExpiredCsrf(response)) return response;
    if (!await refreshCsrfToken()) return response;

    return send();
}

/**
 * POST form data with CSRF token.
 * @param {string} url - Request URL
 * @returns {Promise<Response>}
 */
export async function postForm(url) {
    return fetchWithCsrf(url, { method: 'POST' });
}

/**
 * POST JSON data with CSRF token.
 * @param {string} url - Request URL
 * @param {object} data - JSON data to send
 * @returns {Promise<Response>}
 */
export async function postJson(url, data) {
    return fetchWithCsrf(url, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json'
        },
        body: JSON.stringify(data)
    });
}
