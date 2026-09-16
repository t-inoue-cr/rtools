/// 座標付きテキスト行。`pdf_oxide` に依存せず単体テストできるようにしている。
/// `y` は PDF 座標の下端で、大きいほどページ上。
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutLine {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub font_size: f32,
    pub heading_level: Option<u8>,
}

impl LayoutLine {
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn center_x(&self) -> f32 {
        self.x + self.width / 2.0
    }

    /// pdf_oxide は PDF 座標（大きい Y がページ上）。`y + height` が視覚的な上端。
    pub fn visual_top(&self) -> f32 {
        self.y + self.height
    }
}

/// 抽出した表。`y` は PDF 座標の下端（大きいほどページ上）。
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutTable {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rows: Vec<Vec<String>>,
    pub has_header: bool,
}

impl LayoutTable {
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn visual_top(&self) -> f32 {
        self.y + self.height
    }
}

/// ページを再構成したブロック。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageBlock {
    Heading(String),
    Paragraph(String),
    Table {
        rows: Vec<Vec<String>>,
        has_header: bool,
    },
}

// Heading / overlap
const HEADING_SIZE_RATIO: f32 = 1.25;
const OVERLAP_DROP: f32 = 0.4;

// Line merge
/// Docling `add_orphan_regions`: 同一行とみなす上端差（行高の比）。
const SAME_LINE_TOP_RATIO: f32 = 0.9;
const MIN_EM: f32 = 1.0;

// Paragraph wrap
const INDENT_BREAK_EM: f32 = 2.0;
/// Docling `recover_text_panels`: 段落区切りは median leading のこの倍数。
const LEADING_BREAK_RATIO: f32 = 1.8;
/// 行送りが極端に小さいときの下限（median 行高の比）。
const MIN_BREAK_HEIGHT_RATIO: f32 = 0.75;
/// 折り返しとみなすため、前行の右端が枠右端から許される不足分（em）。
const WRAP_FILL_EM: f32 = 0.5;
const FRAME_RIGHT_PERCENTILE: f32 = 0.95;

// Table grid
const MIN_GRID_DIM: usize = 2;
const MIN_POPULATED_CELLS: usize = 2;

// Two-column gutter
const MIN_LINES_FOR_GUTTER: usize = 8;
const MIN_PAGE_WIDTH_FOR_GUTTER: f32 = 200.0;
const GUTTER_BAND_RATIO: f32 = 0.08;
const MAX_GUTTER_CROSSING_DENOM: usize = 10;
const MIN_COLUMN_LINES: usize = 3;

// Fallback
const DEFAULT_MEDIAN_SIZE: f32 = 12.0;

/// 行と表から段落・見出し・表ブロックを組み立てる。
pub fn reconstruct_page(
    mut lines: Vec<LayoutLine>,
    mut tables: Vec<LayoutTable>,
) -> Vec<PageBlock> {
    tables.retain(|table| is_real_grid(&table.rows));
    lines.retain(|line| {
        !line.text.trim().is_empty()
            && !tables
                .iter()
                .any(|table| overlap_fraction(line, table) > OVERLAP_DROP)
    });

    if lines.is_empty() && tables.is_empty() {
        return Vec::new();
    }

    let lines = merge_same_lines(lines);
    let median_size = median_f32(lines.iter().map(|line| line.height.max(line.font_size)));
    let gutter = detect_gutter(&lines);
    let items = ordered_items(lines, tables, gutter);
    let leading = median_line_gap(&items);
    merge_items(items, median_size, leading, gutter)
}

pub fn blocks_to_markdown(blocks: &[PageBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        match block {
            PageBlock::Heading(text) => {
                out.push_str("### ");
                out.push_str(text);
            }
            PageBlock::Paragraph(text) => out.push_str(text),
            PageBlock::Table { rows, has_header } => {
                out.push_str(&table_to_markdown(rows, *has_header));
            }
        }
    }
    out
}

pub fn blocks_to_plain(blocks: &[PageBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        match block {
            PageBlock::Heading(text) | PageBlock::Paragraph(text) => out.push_str(text),
            PageBlock::Table { rows, .. } => {
                for (i, row) in rows.iter().enumerate() {
                    if i > 0 {
                        out.push('\n');
                    }
                    for (j, cell) in row.iter().enumerate() {
                        if j > 0 {
                            out.push(' ');
                        }
                        out.push_str(cell);
                    }
                }
            }
        }
    }
    out
}

pub fn table_to_markdown(rows: &[Vec<String>], has_header: bool) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let col_count = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    if col_count == 0 {
        return String::new();
    }

    let mut out = String::new();
    if has_header {
        push_md_row(&mut out, &rows[0], col_count);
    } else {
        push_md_row(&mut out, &[], col_count);
    }
    out.push('\n');
    out.push('|');
    for _ in 0..col_count {
        out.push_str(" --- |");
    }
    let data = if has_header { &rows[1..] } else { rows };
    for row in data {
        out.push('\n');
        push_md_row(&mut out, row, col_count);
    }
    out
}

/// 単語・断片を日本語／ラテンの規則でつなぐ。
pub fn join_fragments<I, S>(parts: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = String::new();
    for part in parts {
        append_fragment(&mut out, part.as_ref());
    }
    out
}

fn ordered_items(
    lines: Vec<LayoutLine>,
    tables: Vec<LayoutTable>,
    gutter: Option<f32>,
) -> Vec<Item> {
    if gutter.is_none() {
        let mut items: Vec<Item> = Vec::with_capacity(lines.len() + tables.len());
        items.extend(lines.into_iter().map(Item::Line));
        items.extend(tables.into_iter().map(Item::Table));
        sort_items(&mut items);
        return items;
    }

    let mut full_width = Vec::new();
    let mut column_tables = Vec::new();
    for table in tables {
        if is_full_width_table(&table, gutter) {
            full_width.push(table);
        } else {
            column_tables.push(table);
        }
    }
    full_width.sort_by(|a, b| b.visual_top().total_cmp(&a.visual_top()));
    column_tables.sort_by(|a, b| b.visual_top().total_cmp(&a.visual_top()));

    let mut remaining = lines;
    remaining.sort_by(|a, b| {
        b.visual_top()
            .total_cmp(&a.visual_top())
            .then_with(|| a.x.total_cmp(&b.x))
    });

    let mut items = Vec::new();
    for table in full_width {
        let line_n = remaining
            .iter()
            .take_while(|line| line.visual_top() > table.visual_top())
            .count();
        let band_lines: Vec<LayoutLine> = remaining.drain(..line_n).collect();
        let table_n = column_tables
            .iter()
            .take_while(|other| other.visual_top() > table.visual_top())
            .count();
        let band_tables: Vec<LayoutTable> = column_tables.drain(..table_n).collect();
        items.extend(order_band(band_lines, band_tables, gutter));
        items.push(Item::Table(table));
    }
    items.extend(order_band(remaining, column_tables, gutter));
    items
}

fn order_band(lines: Vec<LayoutLine>, tables: Vec<LayoutTable>, gutter: Option<f32>) -> Vec<Item> {
    let Some(gutter) = gutter else {
        let mut items: Vec<Item> = Vec::with_capacity(lines.len() + tables.len());
        items.extend(lines.into_iter().map(Item::Line));
        items.extend(tables.into_iter().map(Item::Table));
        sort_items(&mut items);
        return items;
    };

    let mut left = Vec::new();
    let mut right = Vec::new();
    for line in lines {
        if line.center_x() < gutter {
            left.push(Item::Line(line));
        } else {
            right.push(Item::Line(line));
        }
    }
    for table in tables {
        if table.x + table.width / 2.0 < gutter {
            left.push(Item::Table(table));
        } else {
            right.push(Item::Table(table));
        }
    }
    sort_items(&mut left);
    sort_items(&mut right);
    left.extend(right);
    left
}

fn sort_items(items: &mut [Item]) {
    items.sort_by(|a, b| {
        item_visual_top(b)
            .total_cmp(&item_visual_top(a))
            .then_with(|| item_x(a).total_cmp(&item_x(b)))
    });
}

fn merge_same_lines(mut lines: Vec<LayoutLine>) -> Vec<LayoutLine> {
    if lines.len() < 2 {
        return lines;
    }
    lines.sort_by(|a, b| {
        b.visual_top()
            .total_cmp(&a.visual_top())
            .then_with(|| a.x.total_cmp(&b.x))
    });
    let mut rows: Vec<Vec<LayoutLine>> = Vec::new();
    for line in lines {
        if let Some(row) = rows.last_mut()
            && let Some(last) = row.last()
        {
            let h = last.height.max(line.height).max(MIN_EM);
            if (last.visual_top() - line.visual_top()).abs() < h * SAME_LINE_TOP_RATIO {
                row.push(line);
                continue;
            }
        }
        rows.push(vec![line]);
    }

    let mut merged = Vec::new();
    for mut row in rows {
        row.sort_by(|a, b| a.x.total_cmp(&b.x));
        let mut row_merged: Vec<LayoutLine> = Vec::with_capacity(row.len());
        for line in row {
            if row_merged.last().is_some_and(|last| {
                let h = last.height.max(line.height).max(MIN_EM);
                line.x <= last.right() + h
            }) {
                let prev = row_merged.pop().expect("last fragment");
                row_merged.push(join_lines(prev, &line));
            } else {
                row_merged.push(line);
            }
        }
        merged.extend(row_merged);
    }
    merged
}

fn is_same_line(prev: &LayoutLine, next: &LayoutLine) -> bool {
    let h = prev.height.max(next.height).max(MIN_EM);
    let same_line = (prev.visual_top() - next.visual_top()).abs() < h * SAME_LINE_TOP_RATIO;
    let touching = next.x <= prev.right() + h && next.x >= prev.x - h;
    same_line && touching
}

fn median_line_gap(items: &[Item]) -> f32 {
    let mut gaps = Vec::new();
    let mut prev_line: Option<&LayoutLine> = None;
    for item in items {
        match item {
            Item::Table(_) => prev_line = None,
            Item::Line(line) => {
                if let Some(prev) = prev_line {
                    let gap = (prev.y - line.visual_top()).max(0.0);
                    gaps.push(gap);
                }
                prev_line = Some(line);
            }
        }
    }
    median_f32(gaps.into_iter())
}

fn paragraph_break_threshold(leading: f32, median_size: f32) -> f32 {
    let height = median_size.max(MIN_EM);
    (LEADING_BREAK_RATIO * leading.max(0.0)).max(MIN_BREAK_HEIGHT_RATIO * height)
}

fn merge_items(
    items: Vec<Item>,
    median_size: f32,
    leading: f32,
    gutter: Option<f32>,
) -> Vec<PageBlock> {
    let extents = line_extents(&items);
    let mut blocks = Vec::new();
    let mut paragraph: Option<LayoutLine> = None;

    for item in items {
        match item {
            Item::Table(table) => {
                flush_paragraph(&mut paragraph, &mut blocks);
                blocks.push(PageBlock::Table {
                    rows: table.rows,
                    has_header: table.has_header,
                });
            }
            Item::Line(line) => {
                if is_heading(&line, median_size) {
                    flush_paragraph(&mut paragraph, &mut blocks);
                    let text = collapse_inline_ws(&line.text);
                    if !text.is_empty() {
                        blocks.push(PageBlock::Heading(text));
                    }
                    continue;
                }
                match paragraph.take() {
                    Some(prev)
                        if should_continue(
                            &prev,
                            &line,
                            median_size,
                            leading,
                            column_frame_right(&prev, &extents, gutter, median_size)
                                .max(line.right()),
                        ) =>
                    {
                        paragraph = Some(join_wrapped_lines(prev, &line));
                    }
                    Some(prev) => {
                        push_paragraph(prev, &mut blocks);
                        paragraph = Some(line);
                    }
                    None => paragraph = Some(line),
                }
            }
        }
    }
    flush_paragraph(&mut paragraph, &mut blocks);
    blocks
}

fn flush_paragraph(paragraph: &mut Option<LayoutLine>, blocks: &mut Vec<PageBlock>) {
    if let Some(prev) = paragraph.take() {
        push_paragraph(prev, blocks);
    }
}

fn push_paragraph(line: LayoutLine, blocks: &mut Vec<PageBlock>) {
    let text = collapse_inline_ws(&line.text);
    if !text.is_empty() {
        blocks.push(PageBlock::Paragraph(text));
    }
}

fn join_lines(mut prev: LayoutLine, next: &LayoutLine) -> LayoutLine {
    append_fragment(&mut prev.text, &next.text);
    let x1 = prev.right().max(next.right());
    let y0 = prev.y.min(next.y);
    let y1 = prev.bottom().max(next.bottom());
    prev.x = prev.x.min(next.x);
    prev.width = x1 - prev.x;
    prev.y = y0;
    prev.height = y1 - y0;
    prev
}

/// 折り返し結合。続き判定のため、最後の物理行の座標を残す。
fn join_wrapped_lines(mut prev: LayoutLine, next: &LayoutLine) -> LayoutLine {
    append_fragment(&mut prev.text, &next.text);
    prev.x = next.x;
    prev.y = next.y;
    prev.width = next.width;
    prev.height = next.height;
    prev.font_size = next.font_size;
    prev
}

fn should_continue(
    prev: &LayoutLine,
    next: &LayoutLine,
    median_size: f32,
    leading: f32,
    frame_right: f32,
) -> bool {
    if is_heading(next, median_size) {
        return false;
    }
    if is_same_line(prev, next) {
        return true;
    }
    let dy = prev.visual_top() - next.visual_top();
    if dy <= 0.0 {
        return false;
    }
    if next.right() < prev.x {
        return false;
    }
    let em = median_size.max(MIN_EM);
    if next.x - prev.x > INDENT_BREAK_EM * em {
        return false;
    }
    let gap = (prev.y - next.visual_top()).max(0.0);
    if gap > paragraph_break_threshold(leading, median_size) {
        return false;
    }
    frame_right - prev.right() <= WRAP_FILL_EM * em
}

#[derive(Clone, Copy)]
struct LineExtent {
    x: f32,
    right: f32,
    center_x: f32,
}

fn line_extents(items: &[Item]) -> Vec<LineExtent> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Line(line) => Some(LineExtent {
                x: line.x,
                right: line.right(),
                center_x: line.center_x(),
            }),
            Item::Table(_) => None,
        })
        .collect()
}

/// 同じ枠の右端。行番号が乗った外れ値で max が膨らむのを避けるため 95 パーセンタイルにする。
fn column_frame_right(
    line: &LayoutLine,
    extents: &[LineExtent],
    gutter: Option<f32>,
    median_size: f32,
) -> f32 {
    let slack_x = INDENT_BREAK_EM * median_size.max(MIN_EM);
    let mut rights: Vec<f32> = Vec::new();
    for other in extents {
        if !in_same_frame(line, other, gutter, slack_x) {
            continue;
        }
        rights.push(other.right);
    }
    percentile_f32(rights, FRAME_RIGHT_PERCENTILE).max(line.right())
}

fn percentile_f32(mut values: Vec<f32>, p: f32) -> f32 {
    values.retain(|v| v.is_finite());
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let idx = ((values.len() - 1) as f32 * p.clamp(0.0, 1.0)).round() as usize;
    values[idx.min(values.len() - 1)]
}

fn in_same_frame(line: &LayoutLine, other: &LineExtent, gutter: Option<f32>, slack_x: f32) -> bool {
    if let Some(gutter) = gutter {
        let line_span = line.x < gutter && line.right() > gutter;
        let other_span = other.x < gutter && other.right > gutter;
        if line_span || other_span {
            return line_span && other_span;
        }
        return (line.center_x() < gutter) == (other.center_x < gutter);
    }
    (other.x - line.x).abs() <= slack_x
}

fn is_heading(line: &LayoutLine, median_size: f32) -> bool {
    if line.heading_level.is_some() {
        return true;
    }
    let size = line.font_size.max(line.height);
    median_size > 0.0 && size >= HEADING_SIZE_RATIO * median_size
}

fn is_real_grid(rows: &[Vec<String>]) -> bool {
    if rows.len() < MIN_GRID_DIM {
        return false;
    }
    let cols = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    if cols < MIN_GRID_DIM {
        return false;
    }
    let populated = rows
        .iter()
        .filter(|row| {
            row.iter().filter(|cell| !cell.trim().is_empty()).count() >= MIN_POPULATED_CELLS
        })
        .count();
    populated * 2 >= rows.len()
}

fn detect_gutter(lines: &[LayoutLine]) -> Option<f32> {
    if lines.len() < MIN_LINES_FOR_GUTTER {
        return None;
    }
    let mut page_left = f32::INFINITY;
    let mut page_right = f32::NEG_INFINITY;
    for line in lines {
        page_left = page_left.min(line.x);
        page_right = page_right.max(line.right());
    }
    let width = page_right - page_left;
    if !width.is_finite() || width < MIN_PAGE_WIDTH_FOR_GUTTER {
        return None;
    }
    let mid = (page_left + page_right) / 2.0;
    let gutter_lo = mid - width * GUTTER_BAND_RATIO;
    let gutter_hi = mid + width * GUTTER_BAND_RATIO;
    let mut crossing = 0usize;
    let mut left = 0usize;
    let mut right = 0usize;
    for line in lines {
        if line.x < gutter_lo && line.right() > gutter_hi {
            crossing += 1;
        }
        if line.right() <= mid {
            left += 1;
        }
        if line.x >= mid {
            right += 1;
        }
    }
    if crossing <= lines.len() / MAX_GUTTER_CROSSING_DENOM
        && left >= MIN_COLUMN_LINES
        && right >= MIN_COLUMN_LINES
    {
        Some(mid)
    } else {
        None
    }
}

fn is_full_width_table(table: &LayoutTable, gutter: Option<f32>) -> bool {
    let Some(gutter) = gutter else {
        return true;
    };
    table.x < gutter && table.right() > gutter
}

fn overlap_fraction(line: &LayoutLine, table: &LayoutTable) -> f32 {
    let x0 = line.x.max(table.x);
    let y0 = line.y.max(table.y);
    let x1 = line.right().min(table.right());
    let y1 = line.bottom().min(table.bottom());
    let w = (x1 - x0).max(0.0);
    let h = (y1 - y0).max(0.0);
    let area = line.width * line.height;
    if area <= 0.0 { 0.0 } else { (w * h) / area }
}

fn append_fragment(out: &mut String, part: &str) {
    let part = part.trim();
    if part.is_empty() {
        return;
    }
    if out.is_empty() {
        out.push_str(part);
        return;
    }
    while out.chars().last().is_some_and(char::is_whitespace) {
        out.pop();
    }
    let left_last = out.chars().last();
    let right_first = part.chars().next();
    if let (Some('-'), Some(next)) = (left_last, right_first)
        && next.is_ascii_alphabetic()
    {
        out.pop();
        out.push_str(part);
        return;
    }
    if should_insert_space(left_last, right_first) {
        out.push(' ');
    }
    out.push_str(part);
}

fn should_insert_space(left: Option<char>, right: Option<char>) -> bool {
    let (Some(left), Some(right)) = (left, right) else {
        return false;
    };
    if left.is_whitespace() || right.is_whitespace() {
        return false;
    }
    if is_cjk(left) || is_cjk(right) {
        return false;
    }
    if matches!(
        right,
        ',' | '.' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '%'
    ) {
        return false;
    }
    if matches!(left, '(' | '[' | '{' | '"' | '\'') {
        return false;
    }
    true
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch,
        '\u{3000}'..='\u{303F}'
            | '\u{3040}'..='\u{30FF}'
            | '\u{3400}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FF00}'..='\u{FFEF}'
    )
}

fn collapse_inline_ws(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = true;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

fn escape_cell(text: &str) -> String {
    collapse_inline_ws(text).replace('|', "\\|")
}

fn push_md_row(out: &mut String, cells: &[String], col_count: usize) {
    out.push('|');
    for i in 0..col_count {
        out.push(' ');
        if let Some(cell) = cells.get(i) {
            out.push_str(&escape_cell(cell));
        }
        out.push_str(" |");
    }
}

fn median_f32<I>(values: I) -> f32
where
    I: Iterator<Item = f32>,
{
    let mut values: Vec<f32> = values.filter(|v| v.is_finite() && *v > 0.0).collect();
    if values.is_empty() {
        return DEFAULT_MEDIAN_SIZE;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let mid = values.len() / 2;
    if values.len() % 2 == 0 && mid > 0 {
        values[mid - 1]
    } else {
        values[mid]
    }
}

#[derive(Debug, Clone)]
enum Item {
    Line(LayoutLine),
    Table(LayoutTable),
}

fn item_visual_top(item: &Item) -> f32 {
    match item {
        Item::Line(line) => line.visual_top(),
        Item::Table(table) => table.visual_top(),
    }
}

fn item_x(item: &Item) -> f32 {
    match item {
        Item::Line(line) => line.x,
        Item::Table(table) => table.x,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x: f32, y: f32, width: f32, height: f32, font_size: f32) -> LayoutLine {
        LayoutLine {
            text: text.into(),
            x,
            y,
            width,
            height,
            font_size,
            heading_level: None,
        }
    }

    fn table(y: f32, height: f32, rows: Vec<Vec<&str>>) -> LayoutTable {
        LayoutTable {
            x: 40.0,
            y,
            width: 400.0,
            height,
            rows: rows
                .into_iter()
                .map(|row| row.into_iter().map(str::to_string).collect())
                .collect(),
            has_header: true,
        }
    }

    #[test]
    fn joins_japanese_wrapped_lines() {
        let lines = [
            line("これは折り返された", 50.0, 700.0, 200.0, 14.0, 12.0),
            line("文章です。", 50.0, 684.0, 80.0, 14.0, 12.0),
        ];
        let blocks = reconstruct_page(lines.into(), Vec::new());
        assert_eq!(
            blocks,
            vec![PageBlock::Paragraph("これは折り返された文章です。".into())]
        );
    }

    #[test]
    fn joins_wrapped_lines_with_wide_leading() {
        let lines = [
            line("特性排気速度効率を向", 50.0, 700.0, 200.0, 14.0, 12.0),
            line("上できるインジェクタ。", 50.0, 672.0, 180.0, 14.0, 12.0),
        ];
        let blocks = reconstruct_page(lines.into(), Vec::new());
        assert_eq!(
            blocks,
            vec![PageBlock::Paragraph(
                "特性排気速度効率を向上できるインジェクタ。".into()
            )]
        );
    }

    #[test]
    fn joins_after_japanese_period_when_leading_matches() {
        let lines = [
            line("これは一文です。", 50.0, 700.0, 160.0, 14.0, 12.0),
            line("次の文です。", 50.0, 684.0, 140.0, 14.0, 12.0),
        ];
        let blocks = reconstruct_page(lines.into(), Vec::new());
        assert_eq!(
            blocks,
            vec![PageBlock::Paragraph("これは一文です。次の文です。".into())]
        );
    }

    #[test]
    fn splits_when_previous_line_does_not_fill_frame() {
        let lines = [
            line("短い段落。", 50.0, 700.0, 80.0, 14.0, 12.0),
            line("次の段落は長い行です。", 50.0, 684.0, 200.0, 14.0, 12.0),
        ];
        let blocks = reconstruct_page(lines.into(), Vec::new());
        assert_eq!(
            blocks,
            vec![
                PageBlock::Paragraph("短い段落。".into()),
                PageBlock::Paragraph("次の段落は長い行です。".into()),
            ]
        );
    }

    #[test]
    fn splits_after_short_wrap_before_next_paragraph() {
        let lines = [
            line("これは折り返された長い", 50.0, 700.0, 200.0, 14.0, 12.0),
            line("段落の終わり。", 50.0, 684.0, 90.0, 14.0, 12.0),
            line("新しい段落の始まりは長い行", 50.0, 668.0, 200.0, 14.0, 12.0),
        ];
        let blocks = reconstruct_page(lines.into(), Vec::new());
        assert_eq!(
            blocks,
            vec![
                PageBlock::Paragraph("これは折り返された長い段落の終わり。".into()),
                PageBlock::Paragraph("新しい段落の始まりは長い行".into()),
            ]
        );
    }

    #[test]
    fn splits_when_vertical_gap_exceeds_leading() {
        let mut lines = Vec::new();
        for i in 0..3 {
            let y = 700.0 - i as f32 * 16.0;
            lines.push(line(&format!("上{i}。"), 50.0, y, 140.0, 14.0, 12.0));
        }
        for i in 0..3 {
            let y = 520.0 - i as f32 * 16.0;
            lines.push(line(&format!("下{i}。"), 50.0, y, 140.0, 14.0, 12.0));
        }
        let blocks = reconstruct_page(lines, Vec::new());
        assert_eq!(
            blocks,
            vec![
                PageBlock::Paragraph("上0。上1。上2。".into()),
                PageBlock::Paragraph("下0。下1。下2。".into()),
            ]
        );
    }

    #[test]
    fn joins_same_visual_line_fragments() {
        let lines = [
            line("第１の端部、", 50.0, 700.0, 90.0, 14.0, 12.0),
            line("第２の端部、", 145.0, 701.0, 90.0, 14.0, 12.0),
            line("チャンバを囲む", 240.0, 699.5, 100.0, 14.0, 12.0),
        ];
        let blocks = reconstruct_page(lines.into(), Vec::new());
        assert_eq!(
            blocks,
            vec![PageBlock::Paragraph(
                "第１の端部、第２の端部、チャンバを囲む".into()
            )]
        );
    }

    #[test]
    fn keeps_latin_word_spaces() {
        let lines = [
            line("This is a wrapped", 50.0, 700.0, 180.0, 14.0, 12.0),
            line("English sentence.", 50.0, 684.0, 140.0, 14.0, 12.0),
        ];
        let blocks = reconstruct_page(lines.into(), Vec::new());
        assert_eq!(
            blocks,
            vec![PageBlock::Paragraph(
                "This is a wrapped English sentence.".into()
            )]
        );
    }

    #[test]
    fn table_markdown_escapes_pipes() {
        let rows = vec![
            vec!["項目".into(), "値".into()],
            vec!["A|B".into(), "1".into()],
        ];
        let md = table_to_markdown(&rows, true);
        assert_eq!(md, "| 項目 | 値 |\n| --- | --- |\n| A\\|B | 1 |");
    }

    #[test]
    fn drops_lines_overlapping_table() {
        let lines = [
            line("見出しの前", 50.0, 400.0, 120.0, 14.0, 12.0),
            line("表の中の文字", 50.0, 120.0, 120.0, 14.0, 12.0),
            line("表のあと", 50.0, 50.0, 120.0, 14.0, 12.0),
        ];
        let tables = [table(
            100.0,
            120.0,
            vec![vec!["項目", "値"], vec!["A", "1"]],
        )];
        let blocks = reconstruct_page(lines.into(), tables.into());
        assert_eq!(
            blocks,
            vec![
                PageBlock::Paragraph("見出しの前".into()),
                PageBlock::Table {
                    rows: vec![
                        vec!["項目".into(), "値".into()],
                        vec!["A".into(), "1".into()],
                    ],
                    has_header: true,
                },
                PageBlock::Paragraph("表のあと".into()),
            ]
        );
        let md = blocks_to_markdown(&blocks);
        assert!(md.contains("| 項目 | 値 |"));
        assert!(!md.contains("表の中の文字"));
    }

    #[test]
    fn dehyphenates_latin_wrap() {
        assert_eq!(join_fragments(["inter-", "national"]), "international");
    }

    #[test]
    fn heading_from_font_size() {
        let lines = [
            line("大きな見出し", 50.0, 720.0, 180.0, 22.0, 22.0),
            line("本文の一行目です。", 50.0, 690.0, 180.0, 14.0, 12.0),
        ];
        let blocks = reconstruct_page(lines.into(), Vec::new());
        assert_eq!(
            blocks,
            vec![
                PageBlock::Heading("大きな見出し".into()),
                PageBlock::Paragraph("本文の一行目です。".into()),
            ]
        );
        assert_eq!(
            blocks_to_markdown(&blocks),
            "### 大きな見出し\n\n本文の一行目です。"
        );
    }

    #[test]
    fn join_fragments_handles_cjk_and_latin() {
        assert_eq!(join_fragments(["価格は", "100円"]), "価格は100円");
        assert_eq!(join_fragments(["hello", "world"]), "hello world");
    }

    #[test]
    fn join_fragments_scales_to_many_chars() {
        let parts: Vec<String> = (0..2000).map(|_| "あ".to_string()).collect();
        let joined = join_fragments(parts.iter().map(String::as_str));
        assert_eq!(joined.chars().count(), 2000);
        assert!(joined.chars().all(|ch| ch == 'あ'));
    }

    #[test]
    fn two_column_reads_left_then_right() {
        let mut lines = Vec::new();
        for i in 0..4 {
            let y = 700.0 - i as f32 * 16.0;
            lines.push(line(&format!("L{i}"), 20.0, y, 80.0, 14.0, 12.0));
            lines.push(line(&format!("R{i}"), 300.0, y, 80.0, 14.0, 12.0));
        }
        let blocks = reconstruct_page(lines, Vec::new());
        let plain = blocks_to_plain(&blocks);
        let l_pos = plain.find("L0").unwrap();
        let r_pos = plain.find("R0").unwrap();
        assert!(l_pos < r_pos, "unexpected order: {plain}");
        assert!(plain.contains("L3"));
        assert!(
            !plain.contains("L0 R0"),
            "columns were interleaved: {plain}"
        );
    }

    #[test]
    fn reading_order_follows_pdf_y_up() {
        let lines = [
            line("ページ下部", 50.0, 80.0, 120.0, 14.0, 12.0),
            line("ページ上部", 50.0, 700.0, 120.0, 14.0, 12.0),
        ];
        let plain = blocks_to_plain(&reconstruct_page(lines.into(), Vec::new()));
        let top = plain.find("ページ上部").unwrap();
        let bottom = plain.find("ページ下部").unwrap();
        assert!(top < bottom, "PDF Y-up was inverted: {plain}");
    }

    #[test]
    fn plain_tables_join_cells_with_spaces() {
        let blocks = [PageBlock::Table {
            rows: vec![vec!["A".into(), "B".into()], vec!["1".into(), "2".into()]],
            has_header: true,
        }];
        assert_eq!(blocks_to_plain(&blocks), "A B\n1 2");
    }
}
