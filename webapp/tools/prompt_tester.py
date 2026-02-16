#!/usr/bin/env python3
"""AI prompt testing script for evaluating Ollama models on todo parsing."""

import argparse
import json
import os
import sys
import time
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from typing import Any, Optional

import requests


# ANSI color codes
class Colors:
    RESET = "\033[0m"
    BOLD = "\033[1m"
    RED = "\033[91m"
    GREEN = "\033[92m"
    YELLOW = "\033[93m"
    BLUE = "\033[94m"
    MAGENTA = "\033[95m"
    CYAN = "\033[96m"
    WHITE = "\033[97m"
    DIM = "\033[2m"


# Test cases with expected outcomes
@dataclass
class TestCase:
    input: str
    expected_title_contains: Optional[str]
    expected_due: Optional[str]  # 'tomorrow', 'next_monday', None, or specific date
    expected_context: Optional[str]  # Expected context or None for any
    expected_project: Optional[str]  # Expected project or None for any
    should_reject: bool = False


TEST_CASES = [
    # Valid todos (7)
    TestCase(
        input="Milch kaufen",
        expected_title_contains="Milch",
        expected_due=None,
        expected_context="einkaufen",
        expected_project=None,
    ),
    TestCase(
        input="Morgen Zahnarzt um 14 Uhr",
        expected_title_contains="Zahnarzt",
        expected_due="tomorrow_14",
        expected_context="termin",
        expected_project=None,
    ),
    TestCase(
        input="Für die Arbeit Präsentation vorbereiten",
        expected_title_contains="Präsentation",
        expected_due=None,
        expected_context="arbeit",
        expected_project=None,
    ),
    TestCase(
        input="Im Supermarkt Brot kaufen",
        expected_title_contains="Brot",
        expected_due=None,
        expected_context="einkaufen",
        expected_project=None,
    ),
    TestCase(
        input="Ich muss noch die Steuererklärung machen",
        expected_title_contains="Steuer",
        expected_due=None,
        expected_context=None,
        expected_project=None,
    ),
    TestCase(
        input="Nächsten Montag mit Lisa über das Projekt sprechen",
        expected_title_contains="Lisa",
        expected_due="next_monday",
        expected_context=None,
        expected_project=None,
    ),
    TestCase(
        input="Ruf Mama an wegen Geburtstag",
        expected_title_contains="Mama",
        expected_due=None,
        expected_context="telefon",
        expected_project=None,
    ),
    # Non-todos (3) - must be rejected
    TestCase(
        input="Das Wetter ist heute schön",
        expected_title_contains=None,
        expected_due=None,
        expected_context=None,
        expected_project=None,
        should_reject=True,
    ),
    TestCase(
        input="Ich glaube vielleicht gehe ich schwimmen",
        expected_title_contains=None,
        expected_due=None,
        expected_context=None,
        expected_project=None,
        should_reject=True,
    ),
    TestCase(
        input="Wann hat der Laden auf?",
        expected_title_contains=None,
        expected_due=None,
        expected_context=None,
        expected_project=None,
        should_reject=True,
    ),
]


def get_today_info() -> tuple[str, str]:
    """Return today's date and weekday."""
    now = datetime.now()
    return now.strftime("%Y-%m-%d"), now.strftime("%A")


def get_next_monday() -> str:
    """Get the date of next Monday."""
    today = datetime.now()
    days_ahead = 7 - today.weekday()  # Monday is 0
    if days_ahead <= 0:
        days_ahead += 7
    next_mon = today + timedelta(days=days_ahead)
    return next_mon.strftime("%Y-%m-%d")


def get_tomorrow() -> str:
    """Get tomorrow's date."""
    return (datetime.now() + timedelta(days=1)).strftime("%Y-%m-%d")


# Prompts from ai_service.py
def build_verbose_prompt(today: str, weekday: str, projects: list[str], contexts: list[str]) -> str:
    """Build the detailed prompt for large models."""
    prompt = (
        f"Parse todo items. Today: {weekday}, {today}.\n\n"
        "TASK: Extract structured data from input OR reject if not actionable.\n\n"
        "VALID: tasks, intentions ('ich muss...'), plans with dates, reminders.\n"
        "INVALID: speculative thoughts ('ich glaube vielleicht...'), observations, questions, chat.\n\n"
        "OUTPUT JSON (all keys required):\n"
        '{"rejected": bool, "confidence": 0.0-1.0, "title": str|null, "note": str|null, "due": "YYYY-MM-DDTHH:MM"|null, "context": str, "project": str}\n\n'
        "RULES: JSON only. Keep input language. German tags. ALWAYS assign context and project. If existing tag matches (case-insensitive), use exact existing case.\n\n"
        'Example: \'Kaufe morgen Milch\' -> {"rejected": false, "confidence": 0.95, "title": "Kaufe Milch", "note": null, "due": "2026-01-18T00:00", "context": "einkaufen", "project": "haushalt"}\n'
        'Example: \'ich glaube ich gehe schwimmen\' -> {"rejected": true, "confidence": 0.9, "title": null, "note": null, "due": null, "context": null, "project": null}'
    )

    tag_hints = []
    if projects:
        tag_hints.append("Existing projects: " + ", ".join(projects))
    if contexts:
        tag_hints.append("Existing contexts: " + ", ".join(contexts))
    if tag_hints:
        prompt += "\n\nPrefer these tags: " + ". ".join(tag_hints) + "."

    return prompt


def build_minimal_prompt(today: str, weekday: str, projects: list[str], contexts: list[str]) -> str:
    """Build a simplified prompt for smaller models."""
    prompt = f"Parse the user's text into a todo. Today: {weekday}, {today}.\n"
    prompt += 'Output JSON with these fields:\n'
    prompt += '- title: the task from user input\n'
    prompt += '- due: date as "YYYY-MM-DDTHH:MM" or null\n'
    prompt += '- context: where/how (pick from list below)\n'
    prompt += '- project: topic (pick from list below)\n'

    if projects:
        prompt += "Projects: " + ", ".join(projects) + "\n"
    if contexts:
        prompt += "Contexts: " + ", ".join(contexts) + "\n"

    prompt += 'If not a task, return {"title":null}'

    return prompt


# Default tags for testing (without real user data)
DEFAULT_PROJECTS = ["haushalt", "arbeit", "privat", "gesundheit", "finanzen"]
DEFAULT_CONTEXTS = ["einkaufen", "telefon", "computer", "termin", "arbeit", "zuhause"]


def list_ollama_models(ollama_url: str) -> list[dict[str, Any]]:
    """Query Ollama for available models."""
    try:
        response = requests.get(f"{ollama_url}/api/tags", timeout=10)
        response.raise_for_status()
        data = response.json()
        return data.get("models", [])
    except requests.RequestException as e:
        print(f"{Colors.RED}Error connecting to Ollama: {e}{Colors.RESET}")
        return []


def select_models(models: list[dict[str, Any]]) -> list[str]:
    """Interactive model selection."""
    print(f"\n{Colors.BOLD}Available Models:{Colors.RESET}")
    for i, model in enumerate(models, 1):
        name = model.get("name", "unknown")
        size = model.get("size", 0)
        size_gb = size / (1024**3) if size else 0
        print(f"  {Colors.CYAN}{i:2d}{Colors.RESET}. {name} ({size_gb:.1f} GB)")

    print(f"\n{Colors.BOLD}Select models (comma-separated numbers or 'all'):{Colors.RESET}")
    selection = input("> ").strip()

    if selection.lower() == "all":
        selected = [m["name"] for m in models]
    else:
        try:
            indices = [int(x.strip()) for x in selection.split(",")]
            selected = []
            for idx in indices:
                if 1 <= idx <= len(models):
                    selected.append(models[idx - 1]["name"])
        except ValueError:
            print(f"{Colors.RED}Invalid selection. Using first model.{Colors.RESET}")
            selected = [models[0]["name"]] if models else []

    return selected


def run_ollama_test(
    ollama_url: str,
    model: str,
    system_prompt: str,
    user_input: str,
    timeout: int = 60,
) -> tuple[Optional[dict[str, Any]], float, Optional[str]]:
    """Run a single test against Ollama.

    Returns: (parsed_result, response_time_seconds, error_message)
    """
    chat_url = f"{ollama_url}/api/chat"
    start_time = time.time()

    try:
        response = requests.post(
            chat_url,
            json={
                "model": model,
                "messages": [
                    {"role": "system", "content": system_prompt},
                    {"role": "user", "content": user_input},
                ],
                "stream": False,
                "format": "json",
            },
            timeout=timeout,
        )
        elapsed = time.time() - start_time

        if response.status_code != 200:
            return None, elapsed, f"HTTP {response.status_code}"

        result = response.json()
        raw_response = result["message"]["content"].strip()

        # Strip markdown code blocks if present
        if raw_response.startswith("```"):
            lines = raw_response.splitlines()
            if lines[0].startswith("```"):
                lines = lines[1:]
            if lines and lines[-1].startswith("```"):
                lines = lines[:-1]
            raw_response = "\n".join(lines).strip()

        parsed = json.loads(raw_response)
        return parsed, elapsed, None

    except requests.RequestException as e:
        elapsed = time.time() - start_time
        return None, elapsed, f"Request error: {e}"
    except json.JSONDecodeError as e:
        elapsed = time.time() - start_time
        return None, elapsed, f"JSON error: {e}"


def is_rejected(result: Optional[dict[str, Any]]) -> bool:
    """Check if a result indicates rejection."""
    if result is None:
        return True
    if result.get("rejected", False):
        return True
    if result.get("title") is None:
        return True
    return False


def score_test(
    test_case: TestCase,
    run1: Optional[dict[str, Any]],
    run2: Optional[dict[str, Any]],
    run1_error: Optional[str],
    run2_error: Optional[str],
) -> dict[str, Any]:
    """Score a test case based on two runs.

    Returns scoring breakdown and consistency metrics.
    """
    scores = {
        "json_valid": 0,  # 0-1
        "rejection_correct": 0,  # 0-1
        "title_match": 0,  # 0-3
        "due_match": 0,  # 0-2
        "context_reasonable": 0,  # 0-1
        "project_assigned": 0,  # 0-1
        "consistency": 0,  # 0-5
        "accuracy_total": 0,  # Sum of accuracy scores (max 9)
        "total": 0,  # accuracy + consistency (max 14)
    }

    # JSON Valid (at least one run parsed successfully)
    if run1 is not None or run2 is not None:
        scores["json_valid"] = 1

    # Use the best run for accuracy scoring
    best_run = run1 if run1 is not None else run2

    if best_run is None:
        # Both runs failed - only check if rejection was expected
        if test_case.should_reject:
            scores["rejection_correct"] = 1
        scores["accuracy_total"] = scores["json_valid"] + scores["rejection_correct"]
        scores["total"] = scores["accuracy_total"] + scores["consistency"]
        return scores

    # Rejection correctness
    was_rejected = is_rejected(best_run)
    if test_case.should_reject and was_rejected:
        scores["rejection_correct"] = 1
        # Correctly rejected - give full marks for NOT extracting content
        scores["title_match"] = 3
        scores["due_match"] = 2
        scores["context_reasonable"] = 1
        scores["project_assigned"] = 1
    elif not test_case.should_reject and not was_rejected:
        scores["rejection_correct"] = 1

    # For non-rejected cases, score the content
    if not test_case.should_reject and not was_rejected:
        # Title match (0-3)
        title = best_run.get("title", "")
        if title and test_case.expected_title_contains:
            if test_case.expected_title_contains.lower() in title.lower():
                # Check if it's a clean title (exact or close match)
                if title.lower() == test_case.input.lower():
                    scores["title_match"] = 2  # Exact input preserved
                elif len(title) < len(test_case.input) * 1.5:
                    scores["title_match"] = 3  # Good extraction
                else:
                    scores["title_match"] = 1  # Contains expected but verbose
            else:
                scores["title_match"] = 0
        elif title and not test_case.expected_title_contains:
            scores["title_match"] = 2  # Any title for non-specific cases

        # Due date match (0-2)
        due = best_run.get("due")
        if test_case.expected_due is None and due is None:
            scores["due_match"] = 2
        elif test_case.expected_due is not None and due is not None:
            tomorrow = get_tomorrow()
            next_monday = get_next_monday()

            if test_case.expected_due == "tomorrow_14":
                if due.startswith(tomorrow):
                    scores["due_match"] = 1
                    if "14:00" in due or "14:30" in due:
                        scores["due_match"] = 2
            elif test_case.expected_due == "next_monday":
                if due.startswith(next_monday):
                    scores["due_match"] = 2
                elif due:  # Any future date is partial credit
                    scores["due_match"] = 1
            else:
                # Specific date comparison
                if due.startswith(test_case.expected_due):
                    scores["due_match"] = 2
        elif test_case.expected_due is None and due is not None:
            # Unexpected due date - not penalized heavily
            scores["due_match"] = 1

        # Context reasonable (0-1)
        context = best_run.get("context", "")
        if context:
            if test_case.expected_context:
                if test_case.expected_context.lower() in context.lower():
                    scores["context_reasonable"] = 1
            else:
                # Any context assigned is good
                scores["context_reasonable"] = 1

        # Project assigned (0-1)
        project = best_run.get("project", "")
        if project:
            scores["project_assigned"] = 1

    # Consistency scoring (0-5)
    if run1 is not None and run2 is not None:
        consistency_points = 0

        # Title consistent
        t1 = (run1.get("title") or "").lower()
        t2 = (run2.get("title") or "").lower()
        if t1 == t2 or (t1 and t2 and (t1 in t2 or t2 in t1)):
            consistency_points += 1

        # Due consistent
        d1 = run1.get("due")
        d2 = run2.get("due")
        if d1 == d2:
            consistency_points += 1
        elif d1 and d2 and d1[:10] == d2[:10]:  # Same date, different time
            consistency_points += 1

        # Context consistent
        c1 = (run1.get("context") or "").lower()
        c2 = (run2.get("context") or "").lower()
        if c1 == c2:
            consistency_points += 1

        # Project consistent
        p1 = (run1.get("project") or "").lower()
        p2 = (run2.get("project") or "").lower()
        if p1 == p2:
            consistency_points += 1

        # Rejection consistent
        r1 = is_rejected(run1)
        r2 = is_rejected(run2)
        if r1 == r2:
            consistency_points += 1

        scores["consistency"] = consistency_points
    elif run1 is not None or run2 is not None:
        # One run succeeded, one failed - partial consistency
        scores["consistency"] = 0

    # Calculate totals
    scores["accuracy_total"] = (
        scores["json_valid"]
        + scores["rejection_correct"]
        + scores["title_match"]
        + scores["due_match"]
        + scores["context_reasonable"]
        + scores["project_assigned"]
    )
    scores["total"] = scores["accuracy_total"] + scores["consistency"]

    return scores


def print_table_header(model: str, prompt_name: str):
    """Print the results table header."""
    header = f" Model: {model} | Prompt: {prompt_name} "
    print(f"\n{Colors.BOLD}{Colors.CYAN}{'=' * 80}{Colors.RESET}")
    print(f"{Colors.BOLD}{Colors.WHITE}{header:^80}{Colors.RESET}")
    print(f"{Colors.CYAN}{'=' * 80}{Colors.RESET}")
    print(
        f"{Colors.BOLD}{'Test Case':<25} {'JSON':>5} {'Rej':>5} {'Title':>6} {'Due':>5} "
        f"{'Ctx':>5} {'Cons':>6} {'Total':>8}{Colors.RESET}"
    )
    print(f"{Colors.DIM}{'-' * 80}{Colors.RESET}")


def color_score(score: int, max_score: int, width: int = 0) -> str:
    """Color a score based on percentage, with optional pre-padding."""
    score_str = str(score).rjust(width) if width else str(score)
    if max_score == 0:
        return f"{Colors.DIM}{score_str}{Colors.RESET}"
    pct = score / max_score
    if pct >= 0.8:
        return f"{Colors.GREEN}{score_str}{Colors.RESET}"
    elif pct >= 0.5:
        return f"{Colors.YELLOW}{score_str}{Colors.RESET}"
    else:
        return f"{Colors.RED}{score_str}{Colors.RESET}"


def print_test_result(test_case: TestCase, scores: dict[str, Any]):
    """Print a single test result row."""
    # Truncate test case input for display
    display_input = test_case.input[:22] + "..." if len(test_case.input) > 25 else test_case.input

    json_score = color_score(scores["json_valid"], 1, 5)
    rej_score = color_score(scores["rejection_correct"], 1, 5)
    title_score = color_score(scores["title_match"], 3, 6)
    due_score = color_score(scores["due_match"], 2, 5)
    ctx_score = color_score(scores["context_reasonable"], 1, 5)
    cons_score = color_score(scores["consistency"], 5, 6)
    total_score = color_score(scores["total"], 14, 2)

    print(
        f"{display_input:<25} {json_score} {rej_score} {title_score} {due_score} "
        f"{ctx_score} {cons_score} {total_score}/14"
    )


def print_summary(total_score: int, max_score: int, consistency_pct: float, avg_time: float):
    """Print the summary line."""
    print(f"{Colors.DIM}{'-' * 80}{Colors.RESET}")
    score_color = Colors.GREEN if total_score / max_score >= 0.7 else (Colors.YELLOW if total_score / max_score >= 0.5 else Colors.RED)
    cons_color = Colors.GREEN if consistency_pct >= 80 else (Colors.YELLOW if consistency_pct >= 60 else Colors.RED)
    print(
        f"{Colors.BOLD}Total: {score_color}{total_score}/{max_score}{Colors.RESET} | "
        f"Consistency: {cons_color}{consistency_pct:.0f}%{Colors.RESET} | "
        f"Avg response time: {Colors.CYAN}{avg_time:.1f}s{Colors.RESET}"
    )


def run_tests(
    ollama_url: str,
    models: list[str],
    prompts: dict[str, str],
    output_file: Optional[str] = None,
):
    """Run all tests across models and prompts."""
    today, weekday = get_today_info()

    all_results = []

    for model in models:
        for prompt_name, prompt_template in prompts.items():
            # Build the actual prompt
            if callable(prompt_template):
                system_prompt = prompt_template(today, weekday, DEFAULT_PROJECTS, DEFAULT_CONTEXTS)
            else:
                system_prompt = prompt_template.format(
                    today=today,
                    weekday=weekday,
                    projects=", ".join(DEFAULT_PROJECTS),
                    contexts=", ".join(DEFAULT_CONTEXTS),
                )

            print_table_header(model, prompt_name)

            model_results = {
                "model": model,
                "prompt": prompt_name,
                "total_score": 0,
                "max_score": len(TEST_CASES) * 14,
                "consistency_pct": 0.0,
                "avg_response_time": 0.0,
                "test_results": [],
            }

            total_time = 0.0
            total_consistency = 0
            max_consistency = len(TEST_CASES) * 5

            for test_case in TEST_CASES:
                # Run twice for consistency
                run1, time1, err1 = run_ollama_test(ollama_url, model, system_prompt, test_case.input)
                run2, time2, err2 = run_ollama_test(ollama_url, model, system_prompt, test_case.input)

                total_time += time1 + time2

                scores = score_test(test_case, run1, run2, err1, err2)
                model_results["total_score"] += scores["total"]
                total_consistency += scores["consistency"]

                print_test_result(test_case, scores)

                # Store detailed results
                model_results["test_results"].append({
                    "input": test_case.input,
                    "run1": run1,
                    "run2": run2,
                    "run1_error": err1,
                    "run2_error": err2,
                    "accuracy_score": scores["accuracy_total"],
                    "consistency_score": scores["consistency"],
                    "total_score": scores["total"],
                    "scores_breakdown": scores,
                })

            model_results["avg_response_time"] = total_time / (len(TEST_CASES) * 2)
            model_results["consistency_pct"] = (total_consistency / max_consistency) * 100

            print_summary(
                model_results["total_score"],
                model_results["max_score"],
                model_results["consistency_pct"],
                model_results["avg_response_time"],
            )

            all_results.append(model_results)

    # Find best combination
    best = max(all_results, key=lambda r: (r["total_score"], r["consistency_pct"]))

    # Export results if requested
    if output_file:
        output_data = {
            "timestamp": datetime.now().isoformat(),
            "models_tested": models,
            "prompts_tested": list(prompts.keys()),
            "results": all_results,
            "summary": {
                "best_combination": {
                    "model": best["model"],
                    "prompt": best["prompt"],
                    "score": best["total_score"],
                    "consistency": best["consistency_pct"],
                }
            },
        }
        with open(output_file, "w") as f:
            json.dump(output_data, f, indent=2, ensure_ascii=False)
        print(f"\n{Colors.GREEN}Results exported to: {output_file}{Colors.RESET}")

    # Print overall summary
    print(f"\n{Colors.BOLD}{Colors.CYAN}{'=' * 80}{Colors.RESET}")
    print(f"{Colors.BOLD}{Colors.WHITE}{'OVERALL SUMMARY':^80}{Colors.RESET}")
    print(f"{Colors.CYAN}{'=' * 80}{Colors.RESET}")

    print(f"\n{Colors.BOLD}Best combination:{Colors.RESET}")
    print(f"  Model:  {Colors.GREEN}{best['model']}{Colors.RESET}")
    print(f"  Prompt: {Colors.GREEN}{best['prompt']}{Colors.RESET}")
    print(f"  Score:  {Colors.GREEN}{best['total_score']}/{best['max_score']}{Colors.RESET}")
    print(f"  Consistency: {Colors.GREEN}{best['consistency_pct']:.0f}%{Colors.RESET}")

    print(f"\n{Colors.BOLD}All results:{Colors.RESET}")
    for r in sorted(all_results, key=lambda x: -x["total_score"]):
        pct = r["total_score"] / r["max_score"] * 100
        color = Colors.GREEN if pct >= 70 else (Colors.YELLOW if pct >= 50 else Colors.RED)
        print(
            f"  {r['model']:<20} + {r['prompt']:<12}: "
            f"{color}{r['total_score']:>3}/{r['max_score']}{Colors.RESET} "
            f"({pct:.0f}%) | Cons: {r['consistency_pct']:.0f}%"
        )


def load_custom_prompts(prompts_file: str) -> dict[str, str]:
    """Load custom prompts from a JSON file."""
    try:
        with open(prompts_file, "r") as f:
            data = json.load(f)
        return data.get("prompts", {})
    except (IOError, json.JSONDecodeError) as e:
        print(f"{Colors.RED}Error loading prompts file: {e}{Colors.RESET}")
        return {}


# Gemini API integration for prompt refinement
def call_gemini(
    prompt: str,
    system_prompt: str,
    api_key: str,
    model: str = "gemini-2.0-flash",
    timeout: int = 60,
) -> tuple[Optional[dict[str, Any]], Optional[str]]:
    """Call Gemini API with a prompt.

    Returns: (parsed_result, error_message)
    """
    url = f"https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"

    try:
        response = requests.post(
            url,
            params={"key": api_key},
            json={
                "contents": [{"role": "user", "parts": [{"text": prompt}]}],
                "systemInstruction": {"parts": [{"text": system_prompt}]},
                "generationConfig": {"responseMimeType": "application/json"},
            },
            timeout=timeout,
        )

        if response.status_code != 200:
            return None, f"HTTP {response.status_code}: {response.text[:200]}"

        result = response.json()

        # Extract the generated text
        candidates = result.get("candidates", [])
        if not candidates:
            return None, "No candidates in response"

        parts = candidates[0].get("content", {}).get("parts", [])
        if not parts:
            return None, "No parts in response"

        text = parts[0].get("text", "")
        if not text:
            return None, "Empty response text"

        # Parse the JSON response
        parsed = json.loads(text)
        return parsed, None

    except requests.RequestException as e:
        return None, f"Request error: {e}"
    except json.JSONDecodeError as e:
        return None, f"JSON error: {e}"


def build_refinement_prompt(current_prompt: str, test_results: list[dict[str, Any]]) -> str:
    """Build the prompt for Gemini to analyze results and suggest improvements."""
    # Filter for failures and low scores
    issues = []
    for result in test_results:
        test_input = result.get("input", "")
        total_score = result.get("total_score", 0)
        scores = result.get("scores_breakdown", {})

        # Include tests with less than 70% score (< 10/14)
        if total_score < 10:
            issue = {
                "input": test_input,
                "score": f"{total_score}/14",
                "run1": result.get("run1"),
                "run2": result.get("run2"),
                "problems": [],
            }

            if scores.get("rejection_correct", 0) == 0:
                issue["problems"].append("incorrect rejection decision")
            if scores.get("title_match", 0) < 2:
                issue["problems"].append("poor title extraction")
            if scores.get("due_match", 0) < 2:
                issue["problems"].append("incorrect date parsing")
            if scores.get("context_reasonable", 0) == 0:
                issue["problems"].append("missing/wrong context")
            if scores.get("consistency", 0) < 3:
                issue["problems"].append("inconsistent across runs")

            issues.append(issue)

    issues_text = json.dumps(issues, indent=2, ensure_ascii=False)

    return f"""You are an expert prompt engineer optimizing a system prompt for todo parsing.

Current system prompt:
---
{current_prompt}
---

Test results showing failures and low scores (less than 70%):
{issues_text}

The prompt should:
1. Parse German natural language into structured todo JSON
2. Correctly reject non-actionable input (observations, questions, vague thoughts like "ich glaube vielleicht...")
3. Extract: title, due date (YYYY-MM-DDTHH:MM format), context (where/how), project (topic)
4. Be concise but comprehensive
5. Always calculate dates relative to today's date

IMPORTANT CONSTRAINTS:
- Do NOT add more examples to the prompt. Keep the same number of examples (or fewer).
- Focus on improving the INSTRUCTIONS and RULES, not adding examples.
- Make the prompt more concise if possible.
- Clarify ambiguous rules or edge cases through better wording.

Analyze the failures and suggest an improved prompt. Focus on:
- What patterns are causing failures
- How to make instructions clearer and more precise
- Better rule formulations (not more examples)

Return ONLY valid JSON with this exact structure:
{{"improved_prompt": "your complete improved system prompt here", "changes_made": ["list of specific changes made"]}}"""


def run_refinement_cycle(
    ollama_url: str,
    models: list[str],
    original_results: list[dict[str, Any]],
    prompt_builder: callable,
    prompt_name: str,
    gemini_api_key: str,
    gemini_model: str = "gemini-2.0-flash",
    iteration: int = 1,
) -> tuple[Optional[str], list[dict[str, Any]]]:
    """Run a single refinement cycle using Gemini.

    Returns: (refined_prompt, new_results)
    """
    today, weekday = get_today_info()

    # Get the original prompt text
    original_prompt = prompt_builder(today, weekday, DEFAULT_PROJECTS, DEFAULT_CONTEXTS)

    # Collect all test results for this prompt from all models
    relevant_results = []
    for result in original_results:
        if result.get("prompt") == prompt_name:
            relevant_results.extend(result.get("test_results", []))

    if not relevant_results:
        print(f"{Colors.YELLOW}No results found for prompt '{prompt_name}'{Colors.RESET}")
        return None, []

    print(f"\n{Colors.BOLD}{Colors.CYAN}{'=' * 80}{Colors.RESET}")
    print(f"{Colors.BOLD}{Colors.WHITE}{f'REFINEMENT CYCLE {iteration}':^80}{Colors.RESET}")
    print(f"{Colors.CYAN}{'=' * 80}{Colors.RESET}")

    print(f"\n{Colors.BOLD}Analyzing results with Gemini ({gemini_model})...{Colors.RESET}")

    # Build refinement prompt and call Gemini
    refinement_prompt = build_refinement_prompt(original_prompt, relevant_results)

    gemini_response, error = call_gemini(
        prompt=refinement_prompt,
        system_prompt="You are a prompt engineering expert. Respond only with valid JSON.",
        api_key=gemini_api_key,
        model=gemini_model,
    )

    if error:
        print(f"{Colors.RED}Gemini error: {error}{Colors.RESET}")
        return None, []

    if not gemini_response:
        print(f"{Colors.RED}Empty response from Gemini{Colors.RESET}")
        return None, []

    refined_prompt = gemini_response.get("improved_prompt")
    changes_made = gemini_response.get("changes_made", [])

    if not refined_prompt:
        print(f"{Colors.RED}No improved prompt in Gemini response{Colors.RESET}")
        return None, []

    # Display changes
    print(f"\n{Colors.GREEN}Changes made:{Colors.RESET}")
    for change in changes_made:
        print(f"  - {change}")

    # Display the refined prompt
    print(f"\n{Colors.BOLD}Refined prompt:{Colors.RESET}")
    print(f"{Colors.DIM}{'-' * 80}{Colors.RESET}")
    print(refined_prompt)
    print(f"{Colors.DIM}{'-' * 80}{Colors.RESET}")

    print(f"\n{Colors.BOLD}Testing refined prompt...{Colors.RESET}")

    # Create a dynamic prompt builder for the refined prompt
    def refined_prompt_builder(today: str, weekday: str, projects: list[str], contexts: list[str]) -> str:
        # Replace date placeholders if present
        prompt = refined_prompt
        prompt = prompt.replace("{today}", today)
        prompt = prompt.replace("{weekday}", weekday)
        # Handle any direct date references in the prompt
        return prompt

    refined_prompt_name = f"{prompt_name}_refined_v{iteration}"

    # Run tests with refined prompt
    refined_results = run_tests_internal(
        ollama_url=ollama_url,
        models=models,
        prompts={refined_prompt_name: refined_prompt_builder},
        print_results=True,
    )

    # Display comparison
    print(f"\n{Colors.BOLD}{Colors.CYAN}{'=' * 80}{Colors.RESET}")
    print(f"{Colors.BOLD}{Colors.WHITE}{f'COMPARISON: {prompt_name} vs {refined_prompt_name}':^80}{Colors.RESET}")
    print(f"{Colors.CYAN}{'=' * 80}{Colors.RESET}")

    print(f"\n{Colors.BOLD}{'Model':<25} {'Original':>12} {'Refined':>12} {'Delta':>10}{Colors.RESET}")
    print(f"{Colors.DIM}{'-' * 60}{Colors.RESET}")

    total_original = 0
    total_refined = 0
    max_score = len(TEST_CASES) * 14

    for model in models:
        # Find original score for this model
        orig_score = 0
        for r in original_results:
            if r.get("model") == model and r.get("prompt") == prompt_name:
                orig_score = r.get("total_score", 0)
                break

        # Find refined score for this model
        ref_score = 0
        for r in refined_results:
            if r.get("model") == model:
                ref_score = r.get("total_score", 0)
                break

        delta = ref_score - orig_score
        delta_str = f"+{delta}" if delta > 0 else str(delta)
        delta_color = Colors.GREEN if delta > 0 else (Colors.RED if delta < 0 else Colors.DIM)

        print(
            f"{model:<25} {orig_score:>8}/{max_score} {ref_score:>8}/{max_score} "
            f"{delta_color}{delta_str:>10}{Colors.RESET}"
        )

        total_original += orig_score
        total_refined += ref_score

    # Overall delta
    overall_delta = total_refined - total_original
    overall_delta_str = f"+{overall_delta}" if overall_delta > 0 else str(overall_delta)
    overall_delta_color = Colors.GREEN if overall_delta > 0 else (Colors.RED if overall_delta < 0 else Colors.DIM)

    print(f"{Colors.DIM}{'-' * 60}{Colors.RESET}")
    total_max = max_score * len(models)
    print(
        f"{Colors.BOLD}{'Total':<25} {total_original:>8}/{total_max} {total_refined:>8}/{total_max} "
        f"{overall_delta_color}{overall_delta_str:>10}{Colors.RESET}"
    )

    return refined_prompt, refined_results


def run_tests_internal(
    ollama_url: str,
    models: list[str],
    prompts: dict[str, callable],
    print_results: bool = True,
) -> list[dict[str, Any]]:
    """Run all tests across models and prompts (internal version for refinement).

    Returns list of results without exporting.
    """
    today, weekday = get_today_info()

    all_results = []

    for model in models:
        for prompt_name, prompt_template in prompts.items():
            # Build the actual prompt
            if callable(prompt_template):
                system_prompt = prompt_template(today, weekday, DEFAULT_PROJECTS, DEFAULT_CONTEXTS)
            else:
                system_prompt = prompt_template.format(
                    today=today,
                    weekday=weekday,
                    projects=", ".join(DEFAULT_PROJECTS),
                    contexts=", ".join(DEFAULT_CONTEXTS),
                )

            if print_results:
                print_table_header(model, prompt_name)

            model_results = {
                "model": model,
                "prompt": prompt_name,
                "total_score": 0,
                "max_score": len(TEST_CASES) * 14,
                "consistency_pct": 0.0,
                "avg_response_time": 0.0,
                "test_results": [],
            }

            total_time = 0.0
            total_consistency = 0
            max_consistency = len(TEST_CASES) * 5

            for test_case in TEST_CASES:
                # Run twice for consistency
                run1, time1, err1 = run_ollama_test(ollama_url, model, system_prompt, test_case.input)
                run2, time2, err2 = run_ollama_test(ollama_url, model, system_prompt, test_case.input)

                total_time += time1 + time2

                scores = score_test(test_case, run1, run2, err1, err2)
                model_results["total_score"] += scores["total"]
                total_consistency += scores["consistency"]

                if print_results:
                    print_test_result(test_case, scores)

                # Store detailed results
                model_results["test_results"].append({
                    "input": test_case.input,
                    "run1": run1,
                    "run2": run2,
                    "run1_error": err1,
                    "run2_error": err2,
                    "accuracy_score": scores["accuracy_total"],
                    "consistency_score": scores["consistency"],
                    "total_score": scores["total"],
                    "scores_breakdown": scores,
                })

            model_results["avg_response_time"] = total_time / (len(TEST_CASES) * 2)
            model_results["consistency_pct"] = (total_consistency / max_consistency) * 100

            if print_results:
                print_summary(
                    model_results["total_score"],
                    model_results["max_score"],
                    model_results["consistency_pct"],
                    model_results["avg_response_time"],
                )

            all_results.append(model_results)

    return all_results


def main():
    parser = argparse.ArgumentParser(
        description="Test AI prompts for todo parsing across Ollama models"
    )
    parser.add_argument(
        "--ollama-url",
        default="http://localhost:11434",
        help="Ollama server URL (default: http://localhost:11434)",
    )
    parser.add_argument(
        "--prompts-file",
        help="Load custom prompts from JSON file",
    )
    parser.add_argument(
        "--output",
        help="Export results to JSON file",
    )
    parser.add_argument(
        "--list-models",
        action="store_true",
        help="Just list available models and exit",
    )
    parser.add_argument(
        "--models",
        help="Comma-separated list of models to test (skip interactive selection)",
    )
    # Gemini refinement options
    parser.add_argument(
        "--refine",
        action="store_true",
        help="Enable Gemini prompt refinement cycle",
    )
    parser.add_argument(
        "--gemini-api-key",
        help="Gemini API key (or use GEMINI_API_KEY env var)",
    )
    parser.add_argument(
        "--gemini-model",
        default="gemini-2.0-flash",
        help="Gemini model (default: gemini-2.0-flash)",
    )
    parser.add_argument(
        "--refine-iterations",
        type=int,
        default=1,
        help="Number of refinement iterations (default: 1)",
    )

    args = parser.parse_args()

    # List models
    print(f"{Colors.BOLD}Connecting to Ollama at {args.ollama_url}...{Colors.RESET}")
    models = list_ollama_models(args.ollama_url)

    if not models:
        print(f"{Colors.RED}No models found. Is Ollama running?{Colors.RESET}")
        sys.exit(1)

    if args.list_models:
        print(f"\n{Colors.BOLD}Available models:{Colors.RESET}")
        for model in models:
            name = model.get("name", "unknown")
            size = model.get("size", 0)
            size_gb = size / (1024**3) if size else 0
            print(f"  - {name} ({size_gb:.1f} GB)")
        sys.exit(0)

    # Select models
    if args.models:
        selected_models = [m.strip() for m in args.models.split(",")]
        # Validate
        available = [m["name"] for m in models]
        for m in selected_models:
            if m not in available:
                print(f"{Colors.YELLOW}Warning: Model '{m}' not found{Colors.RESET}")
        selected_models = [m for m in selected_models if m in available]
    else:
        selected_models = select_models(models)

    if not selected_models:
        print(f"{Colors.RED}No models selected.{Colors.RESET}")
        sys.exit(1)

    print(f"\n{Colors.GREEN}Selected models: {', '.join(selected_models)}{Colors.RESET}")

    # Validate Gemini API key if refinement is enabled
    gemini_api_key = None
    if args.refine:
        gemini_api_key = args.gemini_api_key or os.environ.get("GEMINI_API_KEY")
        if not gemini_api_key:
            print(f"{Colors.RED}Error: --refine requires Gemini API key.{Colors.RESET}")
            print(f"{Colors.YELLOW}Provide via --gemini-api-key or GEMINI_API_KEY env var.{Colors.RESET}")
            sys.exit(1)

    # Build prompts dictionary
    prompts = {
        "verbose": build_verbose_prompt,
        "minimal": build_minimal_prompt,
    }

    # Load custom prompts if provided
    if args.prompts_file:
        custom = load_custom_prompts(args.prompts_file)
        prompts.update(custom)
        print(f"{Colors.GREEN}Loaded custom prompts: {', '.join(custom.keys())}{Colors.RESET}")

    # Run initial tests
    if args.refine:
        # Use internal version to get results for refinement
        all_results = run_tests_internal(args.ollama_url, selected_models, prompts, print_results=True)

        # Print initial summary
        if all_results:
            best = max(all_results, key=lambda r: (r["total_score"], r["consistency_pct"]))
            print(f"\n{Colors.BOLD}{Colors.CYAN}{'=' * 80}{Colors.RESET}")
            print(f"{Colors.BOLD}{Colors.WHITE}{'INITIAL RESULTS SUMMARY':^80}{Colors.RESET}")
            print(f"{Colors.CYAN}{'=' * 80}{Colors.RESET}")
            print(f"\n{Colors.BOLD}Best combination:{Colors.RESET}")
            print(f"  Model:  {Colors.GREEN}{best['model']}{Colors.RESET}")
            print(f"  Prompt: {Colors.GREEN}{best['prompt']}{Colors.RESET}")
            print(f"  Score:  {Colors.GREEN}{best['total_score']}/{best['max_score']}{Colors.RESET}")

        # Run refinement cycles
        refined_prompts = {}
        refinement_results = []

        for iteration in range(1, args.refine_iterations + 1):
            # Refine the verbose prompt (typically the main one to optimize)
            refined_prompt, refined_result = run_refinement_cycle(
                ollama_url=args.ollama_url,
                models=selected_models,
                original_results=all_results,
                prompt_builder=build_verbose_prompt,
                prompt_name="verbose",
                gemini_api_key=gemini_api_key,
                gemini_model=args.gemini_model,
                iteration=iteration,
            )

            if refined_prompt:
                refined_prompts[f"verbose_refined_v{iteration}"] = refined_prompt
                refinement_results.extend(refined_result)

                # Use refined results as base for next iteration
                if iteration < args.refine_iterations:
                    # Add refined results to all_results for next iteration
                    all_results.extend(refined_result)

        # Export results if requested
        if args.output:
            output_data = {
                "timestamp": datetime.now().isoformat(),
                "models_tested": selected_models,
                "prompts_tested": list(prompts.keys()),
                "results": all_results,
                "refinement": {
                    "enabled": True,
                    "gemini_model": args.gemini_model,
                    "iterations": args.refine_iterations,
                    "refined_prompts": refined_prompts,
                    "refined_results": refinement_results,
                },
                "summary": {
                    "best_combination": {
                        "model": best["model"] if all_results else None,
                        "prompt": best["prompt"] if all_results else None,
                        "score": best["total_score"] if all_results else 0,
                        "consistency": best["consistency_pct"] if all_results else 0,
                    }
                },
            }
            with open(args.output, "w") as f:
                json.dump(output_data, f, indent=2, ensure_ascii=False)
            print(f"\n{Colors.GREEN}Results exported to: {args.output}{Colors.RESET}")
    else:
        # Standard test run without refinement
        run_tests(args.ollama_url, selected_models, prompts, args.output)


if __name__ == "__main__":
    main()
