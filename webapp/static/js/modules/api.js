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
 * Make a fetch request with CSRF token.
 * @param {string} url - Request URL
 * @param {object} options - Fetch options
 * @returns {Promise<Response>}
 */
export async function fetchWithCsrf(url, options = {}) {
    const headers = options.headers || {};
    headers['X-CSRFToken'] = csrfToken;

    return fetch(url, {
        ...options,
        headers
    });
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
