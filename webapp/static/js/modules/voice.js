/**
 * Voice input via Web Speech API.
 */

let recognition = null;
let voiceBtn = null;
let addInput = null;
let addForm = null;

/**
 * Initialize voice input.
 * @param {object} config - Configuration
 * @param {string} config.language - Recognition language
 */
export function initVoice(config = {}) {
    voiceBtn = document.getElementById('voice-btn');
    addInput = document.getElementById('add-input');
    addForm = document.getElementById('addForm');

    // Disable voice input on mobile; allow on desktop only
    const isMobile = /Android|webOS|iPhone|iPad|iPod|BlackBerry|IEMobile|Opera Mini/i.test(navigator.userAgent);

    if (!isMobile && ('webkitSpeechRecognition' in window || 'SpeechRecognition' in window)) {
        const SpeechRecognition = window.SpeechRecognition || window.webkitSpeechRecognition;
        recognition = new SpeechRecognition();

        recognition.lang = config.language || 'en';
        recognition.interimResults = false;
        recognition.maxAlternatives = 1;

        if (voiceBtn) {
            voiceBtn.style.display = 'flex';
            voiceBtn.onclick = toggleRecording;
        }

        recognition.onstart = () => {
            if (voiceBtn) voiceBtn.classList.add('recording');
        };

        recognition.onend = () => {
            if (voiceBtn) voiceBtn.classList.remove('recording');
        };

        recognition.onresult = (event) => {
            const transcript = event.results[0][0].transcript;
            if (addInput) {
                addInput.value = transcript;
            }
            // Auto-submit form
            if (addForm) {
                addForm.dispatchEvent(new Event('submit', { cancelable: true, bubbles: true }));
            }
        };

        recognition.onerror = (event) => {
            console.error('Speech recognition error:', event.error);
            if (voiceBtn) voiceBtn.classList.remove('recording');
        };
    }
}

function toggleRecording() {
    if (!recognition) return;

    if (voiceBtn && voiceBtn.classList.contains('recording')) {
        recognition.stop();
    } else {
        recognition.start();
    }
}
