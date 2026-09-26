// Parity tests for extension/lib/format.js against the Rust test suites:
// core/src/parser.rs, core/src/renderer.rs, core/src/todo.rs.
//
// Run with: gjs -m extension/tests/run.js

import System from 'system';

import {
    addMonths,
    applyCompletionMarker,
    extractTitle,
    findLineByMarker,
    formatDue,
    markerOf,
    nextDueDate,
    normalizeToken,
    parseLine,
    renderLine,
    rewriteDue,
    rewriteLine,
    splitLines,
    joinLines,
    toggleInLines,
    unescapeNote,
    escapeNote,
} from '../lib/format.js';

let passed = 0;
let failed = 0;

function assertEq(actual, expected, name) {
    const a = JSON.stringify(actual);
    const e = JSON.stringify(expected);
    if (a === e) {
        passed++;
    } else {
        failed++;
        print(`FAIL ${name}\n  actual:   ${a}\n  expected: ${e}`);
    }
}

function assertTrue(cond, name) {
    if (cond) {
        passed++;
    } else {
        failed++;
        print(`FAIL ${name}`);
    }
}

function ymd(d) {
    const p = n => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

const today = ymd(new Date());

// ---------------------------------------------------------- parser.rs cases

assertEq(extractTitle('+Steuererklärung erledigen due:2026-03-03T00:00 ^85rlhbg3'),
    'erledigen', 'extract_title_strips_leading_project');
assertEq(extractTitle('+a +b @c buy milk due:2026-01-01'),
    'buy milk', 'extract_title_strips_multiple_leading_tags');
assertEq(extractTitle('buy milk +groceries @shop'),
    'buy milk', 'extract_title_keeps_tags_inside_title');
assertEq(extractTitle('Just a title'), 'Just a title', 'extract_title_plain');
assertEq(extractTitle('Buy groceries due:2026-01-20'),
    'Buy groceries', 'extract_title_with_due_only');
assertEq(extractTitle('Buy groceries ^abc123'),
    'Buy groceries', 'extract_title_with_marker_only');
assertEq(extractTitle('Buy groceries myday:2026-06-04'),
    'Buy groceries', 'extract_title_stops_at_myday');

{
    const item = parseLine('- [ ] +Steuererklärung erledigen due:2026-03-03T00:00 ^85rlhbg3', 0);
    assertEq(item.title, 'erledigen', 'parse_line_clean_title.title');
    assertEq(item.projects, ['Steuererklärung'], 'parse_line_clean_title.projects');
    assertEq(item.key.marker, '85rlhbg3', 'parse_line_clean_title.marker');
}
{
    const item = parseLine('- [ ] +"Steuererklärung 2024" erledigen due:2026-01-01 ^abc12345', 0);
    assertEq(item.title, 'erledigen', 'parse_line_quoted_project.title');
    assertEq(item.projects, ['Steuererklärung 2024'], 'parse_line_quoted_project_keeps_spaces');
}
{
    const item = parseLine('- [ ] Task @"Home Office" something ^abc12345', 0);
    assertEq(item.contexts, ['Home Office'], 'parse_line_quoted_context_keeps_spaces');
}
{
    const item = parseLine('- [ ] Task +plain +"Name With Spaces" more ^abc12345', 0);
    assertEq(item.projects, ['plain', 'Name With Spaces'],
        'parse_line_mixes_quoted_and_plain_projects');
}
{
    const item = parseLine('- [ ] Task +"Name with \\"quote\\"" ^abc12345', 0);
    assertEq(item.projects, ['Name with "quote"'],
        'parse_line_quoted_project_with_escaped_quote');
}
{
    const item = parseLine('- [ ] Task due:2026-06-10T12:00 myday:2026-06-04 ^abc12345', 0);
    assertEq(ymd(item.myday), '2026-06-04', 'parse_line_extracts_myday');
    assertEq(item.title, 'Task', 'parse_line_extracts_myday.title');
}

// ------------------------------------------------------------ renderer cases

{
    const line = renderLine({
        key: {lineIndex: 0, marker: 'abc12345'},
        title: 'erledigen',
        projects: ['Steuererklärung 2024'],
        contexts: [],
        due: null,
        myday: null,
        reference: null,
        recurrence: null,
        note: null,
        done: false,
    });
    assertTrue(line.includes('+"Steuererklärung 2024"'),
        'render_line_quotes_multi_word_project');
}
{
    // rewrite_line roundtrip: completion marker lands before the ^id.
    const line = '- [ ] Task ^abc12345';
    const done = rewriteLine(line, true);
    assertEq(done, `- [x] Task ✅ ${today} ^abc12345`, 'rewrite_line_done');
    assertTrue(done.split(/\s+/).includes('^abc12345'),
        'rewrite_line_marker_stays_token');
    assertEq(rewriteLine(done, false), line, 'rewrite_line_roundtrip');
}
{
    const done = rewriteLine('- [ ] Task', true);
    assertEq(done, `- [x] Task ✅ ${today}`, 'completion_marker_without_id_appends');
}
{
    assertEq(rewriteLine('- [x] Task ✅ 2020-01-01 ^a1', false),
        '- [ ] Task ^a1', 'rewrite_line_uncheck_strips_completion');
}

// --------------------------------------------------------------- recurrence

{
    // toggle_todos_spawns_next_occurrence_for_recurring
    const lines = splitLines(
        '- [ ] Gießen due:2020-01-01T09:00 rec:daily ^aaa1\n- [ ] Andere ^bbb2\n');
    toggleInLines(lines, [{lineIndex: 0, marker: 'aaa1'}], true);
    assertEq(lines.length, 3, 'recurrence_spawn_appends_line');
    assertTrue(lines[0].startsWith('- [x] Gießen'), 'recurrence_completed_starts_with');
    assertTrue(lines[0].includes(`due:${today}T09:00`),
        'recurrence_overdue_rescheduled_to_today');
    const spawned = lines[2];
    assertTrue(spawned.startsWith('- [ ] Gießen'), 'recurrence_spawned_open');
    assertTrue(spawned.includes('rec:daily'), 'recurrence_spawned_keeps_rule');
    assertTrue(!spawned.includes('^aaa1'), 'recurrence_spawned_fresh_marker');
    const tomorrow = ymd(new Date(Date.now() + 86400000));
    assertTrue(spawned.includes(`due:${tomorrow}T09:00`),
        'recurrence_spawned_due_tomorrow');
}
{
    // toggle_todos_does_not_carry_myday_to_spawned_occurrence
    const lines = splitLines(
        `- [ ] Gießen due:2020-01-01T09:00 myday:${today} rec:daily ^aaa1\n`);
    toggleInLines(lines, [{lineIndex: 0, marker: 'aaa1'}], true);
    assertEq(lines.length, 2, 'myday_spawn_appends_line');
    assertTrue(lines[0].includes(`myday:${today}`), 'myday_kept_on_completed');
    assertTrue(!lines[1].includes('myday:'), 'myday_dropped_on_spawn');
}
{
    // toggle_todos_flips_multiple_and_skips_bad_keys
    const lines = splitLines('- [ ] Eins ^aaa1\n- [ ] Zwei ^bbb2\n');
    toggleInLines(lines,
        [{lineIndex: 0, marker: 'aaa1'}, {lineIndex: 1, marker: 'bbb2'},
            {lineIndex: 0, marker: 'missing'}], true);
    assertEq(lines.join('\n').match(/- \[x\]/g).length, 2, 'toggle_multiple_checks');
    assertEq(lines.join('\n').match(/✅/g).length, 2, 'toggle_multiple_completions');
    toggleInLines(lines, [{lineIndex: 0, marker: 'aaa1'}], false);
    assertTrue(lines[0].includes('- [ ] Eins'), 'toggle_reopen_unchecks');
    assertTrue(lines[1].includes('- [x] Zwei'), 'toggle_reopen_leaves_others');
}

// ------------------------------------------------------------------- dates

{
    // add_months month-end clamping (todo.rs add_months)
    assertEq(ymd(addMonths(new Date(2026, 0, 31), 1)), '2026-02-28', 'add_months_jan31');
    assertEq(ymd(addMonths(new Date(2024, 0, 31), 1)), '2024-02-29', 'add_months_leap');
    assertEq(ymd(addMonths(new Date(2026, 2, 31), 1)), '2026-04-30', 'add_months_mar31');
}
{
    // next_due_date loops past today (todo.rs next_due_date)
    const nd = nextDueDate(new Date(2020, 0, 1, 9, 0), 'daily');
    assertEq(ymd(nd), ymd(new Date(Date.now() + 86400000)),
        'next_due_daily_from_past');
    assertEq(nd.getHours(), 9, 'next_due_keeps_time');
    assertEq(nextDueDate(null, 'weekly') !== null, true, 'next_due_null_due_weekly');
    assertEq(nextDueDate(new Date(2026, 5, 10), 'bogus'), null, 'next_due_unknown_rule');
}

// ------------------------------------------------- line splitting / markers

{
    const content = '- [ ] A ^aaa1\n- [ ] B ^bbb2\n';
    const lines = splitLines(content);
    assertEq(lines.length, 2, 'splitLines_drops_trailing_empty');
    assertEq(joinLines(lines, true), content, 'joinLines_roundtrip');
    assertEq(splitLines('').length, 0, 'splitLines_empty');
}
{
    assertEq(findLineByMarker(['- [ ] A ^aaa1', '- [ ] B ^bbb2'], 'bbb2'), 1,
        'findLineByMarker_own');
    // Obsidian block link before own marker: own marker wins.
    assertEq(markerOf('- [ ] Laufen ^3pip9m ^bbb22222'), 'bbb22222', 'marker_of_last_standalone');
    assertEq(findLineByMarker(['- [ ] X ^3pip9m ^bbb22222'], 'bbb22222'), 0,
        'findLineByMarker_prefers_own');
}
{
    assertEq(normalizeToken('@Foo'), 'Foo', 'normalizeToken_strips_prefix');
    assertEq(normalizeToken('  Bar  '), 'Bar', 'normalizeToken_trims');
    assertEq(normalizeToken('@@@'), null, 'normalizeToken_empty');
}
{
    assertEq(unescapeNote(escapeNote('a "b" \\c\nd')), 'a "b" \\c\nd', 'note_escape_roundtrip');
}

// ------------------------------------------------------- rewriteDue placement

{
    const line = '- [ ] Task +proj ^abc12345';
    const out = rewriteDue(line, new Date(2026, 0, 20, 12, 0));
    assertEq(out, '- [ ] Task due:2026-01-20T12:00 +proj ^abc12345',
        'rewriteDue_inserts_before_fields');
    assertEq(rewriteDue('- [ ] T due:2020-01-01T00:00 ^a1', new Date(2026, 0, 20, 12, 0)),
        '- [ ] T due:2026-01-20T12:00 ^a1', 'rewriteDue_replaces');
}
{
    assertEq(formatDue(new Date(2026, 8, 19, 18, 0)), '2026-09-19T18:00', 'formatDue');
    assertEq(applyCompletionMarker(`- [x] T ✅ ${today} ^a1`, false),
        '- [x] T ^a1', 'applyCompletionMarker_remove');
}

print(`\n${passed} passed, ${failed} failed`);
if (failed > 0)
    System.exit(1);
