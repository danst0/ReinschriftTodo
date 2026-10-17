"""Feedback links: GitHub issue and email, equal options.

Mirror of core/src/feedback.rs. The app sends nothing itself; the links open
the browser or mail program with a prefilled text that ends in version and
browser details.
"""
from urllib.parse import quote

GITHUB_NEW_ISSUE_URL = 'https://github.com/danst0/ReinschriftTodo/issues/new'
FEEDBACK_EMAIL = 'reinschrift-feedback@dumke.me'


def system_info(version: str, lang: str, user_agent: str) -> str:
    lines = [f'Reinschrift: {version}', 'Client: Web', f'Language: {lang}']
    if user_agent:
        lines.append(f'Browser: {user_agent}')
    return '\n'.join(lines)


def body(info: str) -> str:
    return f'\n\n\n---\n{info}\n'


def github_url(info: str) -> str:
    return f"{GITHUB_NEW_ISSUE_URL}?body={quote(body(info), safe='')}"


def mailto_url(info: str) -> str:
    return (f"mailto:{FEEDBACK_EMAIL}?subject={quote('Reinschrift Feedback', safe='')}"
            f"&body={quote(body(info), safe='')}")
