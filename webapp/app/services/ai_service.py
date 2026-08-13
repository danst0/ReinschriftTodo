"""AI parsing service for natural language todo input."""

import json
import logging
import os
import time
from datetime import datetime, timedelta
from typing import Optional, Any

import requests
from flask import current_app

from app.services.todo_service import load_todos
from app.services.date_service import recent_window_bounds
from app.utils.helpers import canonical_casing_map, split_ai_tags

logger = logging.getLogger(__name__)

RECENT_CONTEXT_WINDOW_DAYS = 30


def _normalize_parsed_tags(parsed: dict, recent_context: dict) -> None:
    """Resolve the model's project/context fields into normalized tag lists.

    A plain value stays one name even when it contains spaces ("Big Project");
    only an explicitly prefixed list ("+haushalt +urlaub") is split. Names are
    then snapped onto an existing tag when they only differ in case.
    """
    existing_projects = recent_context.get('topics') or []
    existing_contexts = recent_context.get('locations') or []

    def normalize(values, existing):
        result = []
        for value in values:
            match = next((e for e in existing if e.lower() == value.lower()), value)
            result.append(match)
        return result

    def collect(raw, prefix_char):
        if isinstance(raw, str):
            return split_ai_tags(raw, prefix_char)
        if isinstance(raw, list):
            return [name for item in raw if item for name in split_ai_tags(str(item), prefix_char)]
        return []

    if parsed.get('projects'):
        parsed['projects'] = normalize(collect(parsed['projects'], '+'), existing_projects)
    elif parsed.get('project'):
        parsed['projects'] = normalize(collect(parsed['project'], '+'), existing_projects)

    if parsed.get('contexts'):
        parsed['contexts'] = normalize(collect(parsed['contexts'], '@'), existing_contexts)
    elif parsed.get('context'):
        parsed['contexts'] = normalize(collect(parsed['context'], '@'), existing_contexts)


def collect_recent_context(todos: list, window_days: int = RECENT_CONTEXT_WINDOW_DAYS) -> dict[str, Any]:
    """Collect context from recent todos for AI prompts.

    Args:
        todos: List of all todos.
        window_days: Number of days to look back.

    Returns:
        Context dictionary with topics, locations, and recent todos.
    """
    today, window_start = recent_window_bounds(window_days)

    # Convert to date for comparison
    window_start_date = window_start.date()

    open_todos = [t for t in todos if not (t.done if hasattr(t, 'done') else t.get('done'))]
    recent_closed = []

    for t in todos:
        if hasattr(t, 'done'):
            is_done = t.done
            done_date = t.done_date.date() if t.done_date else None
            raw_line = t.raw_line
        else:
            is_done = t.get('done')
            dd = t.get('done_date')
            done_date = dd.date() if isinstance(dd, datetime) else dd
            raw_line = t.get('raw_line', '')

        if is_done and done_date and done_date >= window_start_date:
            recent_closed.append(t)

    all_relevant = open_todos + recent_closed

    # Extract topics (projects) and locations (contexts); case variants are
    # aggregated to their most frequently used casing.
    raw_topics: list[str] = []
    raw_locations: list[str] = []
    for t in all_relevant:
        if hasattr(t, 'projects'):
            raw_topics.extend(p for p in t.projects if p)
            raw_locations.extend(c for c in t.contexts if c)
        else:
            raw_topics.extend(p for p in t.get('projects', []) if p)
            raw_locations.extend(c for c in t.get('contexts', []) if c)
    topics = set(canonical_casing_map(raw_topics).values())
    locations = set(canonical_casing_map(raw_locations).values())

    recent_full_text = []
    for t in recent_closed:
        if hasattr(t, 'raw_line'):
            if t.raw_line:
                recent_full_text.append(t.raw_line.strip())
        elif t.get('raw_line'):
            recent_full_text.append(t['raw_line'].strip())

    return {
        'today': today,
        'window_start': window_start,
        'topics': sorted(topics),
        'locations': sorted(locations),
        'recent_todos': recent_full_text,
    }


def build_context_reminders(context: dict[str, Any], window_days: int = RECENT_CONTEXT_WINDOW_DAYS) -> str:
    """Build context reminder text for AI prompts.

    Args:
        context: Context dictionary from collect_recent_context.
        window_days: Window size in days.

    Returns:
        Formatted reminder text.
    """
    sections = []

    # Skip closed todos - they make the prompt too long

    topics = context.get('topics') or []
    if topics:
        sections.append("Existing projects: " + ", ".join(topics))

    locations = context.get('locations') or []
    if locations:
        sections.append("Existing contexts: " + ", ".join(locations))

    if not sections:
        return ""

    return "\n\nPrefer these tags: " + ". ".join(sections) + "."


def get_top_tags(todos: list, max_projects: int = 5, max_contexts: int = 5) -> tuple[list[str], list[str]]:
    """Return most frequently used tags from todos.

    Args:
        todos: List of all todos.
        max_projects: Maximum number of projects to return.
        max_contexts: Maximum number of contexts to return.

    Returns:
        Tuple of (top_projects, top_contexts) lists.
    """
    from collections import Counter

    # Count case-insensitively so casing variants aggregate; display the most
    # frequently used casing.
    project_counts: Counter[str] = Counter()
    context_counts: Counter[str] = Counter()
    raw_projects: list[str] = []
    raw_contexts: list[str] = []

    for t in todos:
        if hasattr(t, 'projects'):
            projects = t.projects
            contexts = t.contexts
        else:
            projects = t.get('projects', [])
            contexts = t.get('contexts', [])

        for p in projects:
            if p:
                project_counts[p.lower()] += 1
                raw_projects.append(p)
        for c in contexts:
            if c:
                context_counts[c.lower()] += 1
                raw_contexts.append(c)

    canon_projects = canonical_casing_map(raw_projects)
    canon_contexts = canonical_casing_map(raw_contexts)
    top_projects = [canon_projects.get(tag, tag)
                    for tag, _ in project_counts.most_common(max_projects)]
    top_contexts = [canon_contexts.get(tag, tag)
                    for tag, _ in context_counts.most_common(max_contexts)]

    return top_projects, top_contexts


def build_verbose_prompt(today: str, weekday: str, projects: list[str], contexts: list[str]) -> str:
    """Build the detailed prompt for large models.

    Args:
        today: Today's date in YYYY-MM-DD format.
        weekday: Day of the week (e.g., "Monday").
        projects: List of existing project tags.
        contexts: List of existing context tags.

    Returns:
        Full system prompt string.
    """
    prompt = (
        f"Parse todo items. Today: {weekday}, {today}.\n\n"
        "TASK: Extract structured data from input OR reject if not actionable.\n\n"
        "VALID: tasks, intentions ('ich muss...'), plans with dates, reminders.\n"
        "INVALID: speculative thoughts ('ich glaube vielleicht...'), observations, questions, chat.\n\n"
        "OUTPUT JSON (all keys required):\n"
        '{"rejected": bool, "confidence": 0.0-1.0, "title": str|null, "note": str|null, "due": "YYYY-MM-DDTHH:MM"|null, "context": str, "project": str}\n\n'
        "FIELDS: context = the place or mode where the task happens (e.g. einkaufen, Garten, Computer, Keller). "
        "project = the topic/area the task belongs to, describing what it is about (e.g. Haushalt, Urlaub, Familie). "
        "context and project MUST NOT be the same word.\n\n"
        "RULES: JSON only. Keep input language. German tags. Assign context and (when a fitting tag exists) project. "
        "You MUST reuse an existing tag from the lists below whenever one reasonably fits, instead of inventing a synonym. "
        "Only create a new tag if none of the existing ones fits; then prefer leaving project empty over guessing. "
        "If an existing tag matches (case-insensitive), use the exact existing case.\n\n"
        'Example: \'Kaufe morgen Milch\' -> {"rejected": false, "confidence": 0.95, "title": "Kaufe Milch", "note": null, "due": "2026-01-18T00:00", "context": "einkaufen", "project": "haushalt"}\n'
        'Example: \'ich glaube ich gehe schwimmen\' -> {"rejected": true, "confidence": 0.9, "title": null, "note": null, "due": null, "context": null, "project": null}'
    )

    # Add tag hints if available
    tag_hints = []
    if projects:
        tag_hints.append("Existing projects: " + ", ".join(projects))
    if contexts:
        tag_hints.append("Existing contexts: " + ", ".join(contexts))
    if tag_hints:
        prompt += "\n\nReuse these existing tags whenever one fits: " + ". ".join(tag_hints) + "."

    return prompt


def build_minimal_prompt(today: str, weekday: str, projects: list[str], contexts: list[str]) -> str:
    """Build a simplified prompt for smaller models.

    Args:
        today: Today's date in YYYY-MM-DD format.
        weekday: Day of the week (e.g., "Monday").
        projects: List of top project tags (limited).
        contexts: List of top context tags (limited).

    Returns:
        Minimal system prompt string (~200 tokens).
    """
    prompt = f"Parse the user's text into a todo. Today: {weekday}, {today}.\n"
    prompt += 'Output JSON with these fields:\n'
    prompt += '- title: the task from user input\n'
    prompt += '- due: date as "YYYY-MM-DDTHH:MM" or null\n'
    prompt += '- context: place/mode where the task happens, e.g. einkaufen, Garten (pick from list below)\n'
    prompt += '- project: topic/area the task is about, e.g. Haushalt, Urlaub (pick from list below)\n'
    prompt += '- context and project must not be the same word\n'

    if projects:
        prompt += "Projects: " + ", ".join(projects) + "\n"
    if contexts:
        prompt += "Contexts: " + ", ".join(contexts) + "\n"

    prompt += 'If not a task, return {"title":null}'

    return prompt


def build_system_prompt(today: str, weekday: str, todos: list, style: str = 'verbose') -> str:
    """Build system prompt based on configured style.

    Args:
        today: Today's date in YYYY-MM-DD format.
        weekday: Day of the week (e.g., "Monday").
        todos: List of all todos for context extraction.
        style: Prompt style - 'verbose' or 'minimal'.

    Returns:
        System prompt string.
    """
    if style == 'minimal':
        top_projects, top_contexts = get_top_tags(todos, max_projects=25, max_contexts=25)
        return build_minimal_prompt(today, weekday, top_projects, top_contexts)
    else:
        # For verbose, collect all tags from recent context
        recent_context = collect_recent_context(todos, window_days=RECENT_CONTEXT_WINDOW_DAYS)
        projects = recent_context.get('topics') or []
        contexts = recent_context.get('locations') or []
        return build_verbose_prompt(today, weekday, projects, contexts)


def _get_llm_config() -> dict[str, Any]:
    """Get LLM configuration."""
    app_config = {}

    # Try to load from config.json
    config_file = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(__file__))), 'config.json')
    if os.path.exists(config_file):
        try:
            with open(config_file, 'r') as f:
                app_config = json.load(f)
        except (IOError, json.JSONDecodeError) as e:
            logger.error("Error loading config.json: %s", e)

    # Get from Flask config or environment
    if current_app:
        base_url = current_app.config.get('OLLAMA_URL') or app_config.get('llm_url', "http://10.0.2.71:11434/api/generate")
        model = current_app.config.get('OLLAMA_MODEL') or app_config.get('llm_model', 'llama3.1:8b')
    else:
        base_url = os.environ.get('OLLAMA_URL') or app_config.get('llm_url', "http://10.0.2.71:11434/api/generate")
        model = os.environ.get('OLLAMA_MODEL') or app_config.get('llm_model', 'llama3.1:8b')

    # Construct Chat API URL
    if "/api/generate" in base_url:
        chat_url = base_url.replace("/api/generate", "/api/chat")
    else:
        chat_url = base_url.rstrip('/') + "/api/chat"

    return {
        'chat_url': chat_url,
        'model': model,
    }


def parse_nlp(text: str) -> Optional[dict[str, Any]]:
    """Parse natural language input into structured todo data.

    Args:
        text: Natural language input text.

    Returns:
        Parsed data dictionary or None if parsing fails or input is rejected.
    """
    if not text:
        logger.warning("NLP Parse: No text provided")
        return None

    logger.info("NLP Parse Request: %s", text)

    now = datetime.now()
    today = now.strftime("%Y-%m-%d")
    weekday = now.strftime("%A")

    todos = load_todos()
    recent_context = collect_recent_context(todos, window_days=RECENT_CONTEXT_WINDOW_DAYS)

    config = _get_llm_config()

    # Get prompt style from Flask config
    prompt_style = 'verbose'
    if current_app:
        prompt_style = current_app.config.get('OLLAMA_PROMPT_STYLE', 'verbose')

    system_prompt = build_system_prompt(today, weekday, todos, style=prompt_style)

    try:
        logger.info("Sending request to Ollama (%s) with model %s", config['chat_url'], config['model'])
        logger.debug("System prompt:\n%s", system_prompt)
        logger.debug("User input: %s", text)

        response = requests.post(config['chat_url'], json={
            "model": config['model'],
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": text}
            ],
            "stream": False,
            "format": "json",
            # Deterministic sampling: identical inputs yield identical tags and
            # the model sticks to reusing existing tags.
            "options": {"temperature": 0.0, "top_p": 0.9},
        }, timeout=45)

        logger.info("Ollama response status: %s", response.status_code)
        if response.status_code != 200:
            logger.error("Ollama Error Body: %s", response.text)

        response.raise_for_status()
        result = response.json()
        raw_response = result['message']['content'].strip()
        logger.info("Ollama raw response: %s", raw_response)

        # Strip markdown code blocks if present
        if raw_response.startswith("```"):
            lines = raw_response.splitlines()
            if lines[0].startswith("```"):
                lines = lines[1:]
            if lines and lines[-1].startswith("```"):
                lines = lines[:-1]
            raw_response = "\n".join(lines).strip()

        parsed = json.loads(raw_response)
        logger.info("Parsed JSON: %s", parsed)
        if not isinstance(parsed, dict):
            logger.info("Rejected non-object JSON response: %s", type(parsed).__name__)
            return None

        # Check if input was rejected as non-actionable
        # For verbose mode: check 'rejected' field
        # For minimal mode: check for null/empty title
        if parsed.get('rejected', False):
            logger.info("Input rejected as non-actionable (not a todo/note)")
            return None

        # Handle null title as rejection (minimal prompt style)
        if parsed.get('title') is None and prompt_style == 'minimal':
            logger.info("Input rejected: null title (minimal mode)")
            return None

        # Check confidence threshold (80%) - only for verbose mode
        if prompt_style == 'verbose':
            confidence = parsed.get('confidence', 1.0)
            if confidence < 0.8:
                logger.info("Input rejected due to low confidence: %s", confidence)
                return None

        # Validate/fix keys
        if 'title' not in parsed or not parsed['title']:
            parsed['title'] = text
        if 'note' not in parsed:
            parsed['note'] = None
        if 'due' not in parsed:
            parsed['due'] = None
        if 'contexts' not in parsed:
            parsed['contexts'] = None
        if 'projects' not in parsed:
            parsed['projects'] = None

        if parsed.get('note') == "":
            parsed['note'] = None

        # Normalize tag case to match existing tags
        _normalize_parsed_tags(parsed, recent_context)

        # Remove internal fields from response
        parsed.pop('rejected', None)
        parsed.pop('confidence', None)

        # Auto-fill note with original input text
        parsed['note'] = text

        return parsed

    except requests.RequestException as e:
        logger.error("Ollama request error: %s", e)
        return None
    except json.JSONDecodeError as e:
        logger.error("JSON decode error: %s", e)
        return None
    except Exception as e:
        logger.error("Unexpected error in NLP parsing: %s", e, exc_info=True)
        return None


def parse_nlp_with_debug(text: str) -> dict[str, Any]:
    """Parse natural language input with full debug information.

    Args:
        text: Natural language input text.

    Returns:
        Debug dictionary containing:
        - system_prompt: The full system prompt sent to LLM
        - user_input: The original user text
        - raw_response: The raw string response from Ollama
        - parsed_result: The normalized/parsed result (or None)
        - timing_ms: Request duration in milliseconds
        - model: The model used
        - endpoint: The API endpoint called
        - rejected: Whether the input was rejected
        - confidence: The confidence score (if available)
        - error: Error message if something failed
    """
    debug_info: dict[str, Any] = {
        'system_prompt': None,
        'user_input': text,
        'raw_response': None,
        'parsed_result': None,
        'timing_ms': 0,
        'model': None,
        'endpoint': None,
        'rejected': False,
        'confidence': None,
        'error': None,
    }

    if not text:
        debug_info['error'] = 'No text provided'
        return debug_info

    now = datetime.now()
    today = now.strftime("%Y-%m-%d")
    weekday = now.strftime("%A")

    todos = load_todos()
    recent_context = collect_recent_context(todos, window_days=RECENT_CONTEXT_WINDOW_DAYS)

    config = _get_llm_config()
    debug_info['model'] = config['model']
    debug_info['endpoint'] = config['chat_url']

    # Get prompt style from Flask config
    prompt_style = 'verbose'
    if current_app:
        prompt_style = current_app.config.get('OLLAMA_PROMPT_STYLE', 'verbose')

    debug_info['prompt_style'] = prompt_style

    system_prompt = build_system_prompt(today, weekday, todos, style=prompt_style)
    debug_info['system_prompt'] = system_prompt

    start_time = time.time()

    try:
        response = requests.post(config['chat_url'], json={
            "model": config['model'],
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": text}
            ],
            "stream": False,
            "format": "json",
            # Deterministic sampling: identical inputs yield identical tags and
            # the model sticks to reusing existing tags.
            "options": {"temperature": 0.0, "top_p": 0.9},
        }, timeout=45)

        debug_info['timing_ms'] = int((time.time() - start_time) * 1000)

        if response.status_code != 200:
            debug_info['error'] = f"HTTP {response.status_code}: {response.text}"
            return debug_info

        result = response.json()
        raw_response = result['message']['content'].strip()
        debug_info['raw_response'] = raw_response

        # Strip markdown code blocks if present
        clean_response = raw_response
        if clean_response.startswith("```"):
            lines = clean_response.splitlines()
            if lines[0].startswith("```"):
                lines = lines[1:]
            if lines and lines[-1].startswith("```"):
                lines = lines[:-1]
            clean_response = "\n".join(lines).strip()

        parsed = json.loads(clean_response)

        # Capture rejection status and confidence before processing
        debug_info['rejected'] = parsed.get('rejected', False)
        debug_info['confidence'] = parsed.get('confidence')

        # Check if input was rejected as non-actionable
        # For verbose mode: check 'rejected' field
        # For minimal mode: check for null/empty title
        if parsed.get('rejected', False):
            debug_info['parsed_result'] = None
            return debug_info

        # Handle null title as rejection (minimal prompt style)
        if parsed.get('title') is None and prompt_style == 'minimal':
            debug_info['rejected'] = True
            debug_info['parsed_result'] = None
            return debug_info

        # Check confidence threshold (80%) - only for verbose mode
        if prompt_style == 'verbose':
            confidence = parsed.get('confidence', 1.0)
            if confidence < 0.8:
                debug_info['error'] = f"Low confidence: {confidence}"
                debug_info['parsed_result'] = None
                return debug_info

        # Validate/fix keys
        if 'title' not in parsed or not parsed['title']:
            parsed['title'] = text
        if 'note' not in parsed:
            parsed['note'] = None
        if 'due' not in parsed:
            parsed['due'] = None
        if 'contexts' not in parsed:
            parsed['contexts'] = None
        if 'projects' not in parsed:
            parsed['projects'] = None

        if parsed.get('note') == "":
            parsed['note'] = None

        # Normalize tag case to match existing tags
        _normalize_parsed_tags(parsed, recent_context)

        # Store full parsed result (keep rejected/confidence for debug)
        # Auto-fill note with original input text
        debug_info['parsed_result'] = {
            'title': parsed.get('title'),
            'note': text,  # Always use original input as note
            'due': parsed.get('due'),
            'contexts': parsed.get('contexts', []),
            'projects': parsed.get('projects', []),
        }

        return debug_info

    except requests.RequestException as e:
        debug_info['timing_ms'] = int((time.time() - start_time) * 1000)
        debug_info['error'] = f"Request error: {e}"
        return debug_info
    except json.JSONDecodeError as e:
        debug_info['timing_ms'] = int((time.time() - start_time) * 1000)
        debug_info['error'] = f"JSON decode error: {e}"
        return debug_info
    except Exception as e:
        debug_info['timing_ms'] = int((time.time() - start_time) * 1000)
        debug_info['error'] = f"Unexpected error: {e}"
        return debug_info
