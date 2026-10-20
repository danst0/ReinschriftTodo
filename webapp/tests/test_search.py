"""Tests for the todo search (mirror of core/src/search.rs)."""

import os
import sys

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.services.parser import parse_line
from app.services.search import PRIMARY, SECONDARY, SearchQuery


def search(query: str, line: str):
    return SearchQuery.parse(query).matches(parse_line(line, 0))


class TestSearchQuery:
    def test_title_hit_is_primary(self):
        assert search('Secret', '- [ ] Rotate secrets +Home') == PRIMARY

    def test_hit_only_in_project_or_note_is_secondary(self):
        line = '- [ ] Fix comments +Security @Computer ~note:"sshd config"'
        assert search('sec', line) == SECONDARY
        assert search('comp', line) == SECONDARY
        assert search('sshd', line) == SECONDARY
        assert search('nothing', line) is None

    def test_tag_prefix_filters_by_project_or_place(self):
        line = '- [ ] Fix comments +Security @Computer'
        assert search('+sec', line) == PRIMARY
        assert search('@comp', line) == PRIMARY
        assert search('+comp', line) is None
        assert search('@sec', line) is None
        # A title containing the word does not count for a tag query.
        assert search('+sec', '- [ ] Security audit +Home') is None

    def test_tag_and_text_combine(self):
        line = '- [ ] Limit UFW rule +Security ~note:"octopus"'
        assert search('+security ufw', line) == PRIMARY
        assert search('+security octopus', line) == SECONDARY
        assert search('+home ufw', line) is None

    def test_bare_prefix_matches_any_tag(self):
        assert search('+', '- [ ] A +Home') == PRIMARY
        assert search('+', '- [ ] A') is None

    def test_empty_query_matches_nothing(self):
        assert search('   ', '- [ ] A') is None

    def test_works_on_dicts(self):
        todo = parse_line('- [ ] Fix comments +Security', 0).to_dict()
        assert SearchQuery.parse('sec').matches(todo) == SECONDARY


class TestSearchRoute:
    @pytest.fixture
    def search_client(self, app, tmp_path):
        todo_path = tmp_path / 'todos.md'
        todo_path.write_text(
            '- [ ] Rotate secrets +Home ^aaa111\n'
            '- [ ] Fix misleading comments +Security ^bbb222\n'
            '- [ ] Unrelated task +Home ^ccc333\n',
            encoding='utf-8',
        )
        app.config.update({
            'TODO_PATH': str(todo_path),
            'CONFIG_PATH': str(tmp_path / 'settings.json'),
            'USE_WEBDAV': False,
        })
        client = app.test_client()
        with client.session_transaction() as sess:
            sess['logged_in'] = True
        return client

    def test_secondary_hits_follow_title_hits(self, search_client):
        html = search_client.get('/?q=sec&partial=1').get_data(as_text=True)
        assert 'Rotate secrets' in html
        assert 'Fix misleading comments' in html
        assert 'Unrelated task' not in html
        assert html.index('Rotate secrets') < html.index('Fix misleading comments')

    def test_tag_query_is_primary(self, search_client):
        html = search_client.get('/?q=%2Bsec&partial=1').get_data(as_text=True)
        assert 'Fix misleading comments' in html
        assert 'Rotate secrets' not in html
