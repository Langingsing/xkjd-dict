//! External evidence audit. No local word/code pair is used as pronunciation or
//! stroke evidence. Raw source snapshots are hash-checked before interpretation.
use crate::{
    document::Entry,
    phonetics::{normalize_pinyin, phonetic_codes},
    rules::{Profile, is_han, is_shape, is_sound, profile},
    workspace::Workspace,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SourceId {
    Unihan,
    UnihanVariants,
    Cedict,
    CncharSpell,
    CncharOrder,
    CncharOrderTrad,
    CncharStrokes,
    CncharLicense,
    Hanzi,
    HanziLicense,
    HanziLgpl,
    KeytaoFinals,
    KeytaoRoots,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Snapshot {
    pub id: SourceId,
    pub url: String,
    pub path: PathBuf,
    pub revision: String,
    pub license: String,
    pub lineage: String,
    pub sha256: String,
    pub download_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Reading {
    pub source: SourceId,
    pub raw: String,
    pub normalized: String,
    pub phonetic_codes: BTreeSet<String>,
    pub locator: String,
}

#[derive(Clone, Debug, Deserialize)]
struct Hanzi {
    character: char,
    pinyin: Vec<String>,
    decomposition: String,
    matches: Vec<Option<Vec<usize>>>,
}

#[derive(Clone, Debug, Deserialize)]
struct Stroke {
    name: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct StrokeOrder {
    pub source: SourceId,
    pub ids: String,
    pub five_strokes: Option<String>,
    pub names: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ShapeEvidence {
    pub stroke_orders: Vec<StrokeOrder>,
    pub ids: Option<String>,
    pub ids_source_line: Option<usize>,
    pub matched_strokes: Option<usize>,
    pub unknown_stroke_matches: Option<usize>,
    pub stroke_count_disagreement: bool,
}

#[derive(Debug)]
pub struct ExternalData {
    pub snapshots: Vec<Snapshot>,
    pub readings: BTreeMap<char, Vec<Reading>>,
    orders: BTreeMap<char, Vec<StrokeOrder>>,
    hanzi: BTreeMap<char, (Hanzi, usize)>,
    roots: BTreeMap<char, String>,
    pub root_signatures: Vec<RootSignature>,
    variants: BTreeMap<char, Vec<VariantLink>>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VariantKind {
    Traditional,
    Simplified,
    ZVariant,
}
#[derive(Clone, Debug, Serialize)]
pub struct VariantLink {
    pub character: char,
    pub kind: VariantKind,
    pub locator: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct VariantSupport {
    pub variant: VariantLink,
    pub reading: Reading,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct RootSignature {
    pub root: char,
    pub five_strokes: String,
    pub source_character: char,
    pub order_source: SourceId,
    pub ids_source_line: Option<usize>,
}

impl ExternalData {
    pub fn load(directory: &Path) -> Result<Self> {
        let manifest = directory.join("manifest.json");
        let snapshots: Vec<Snapshot> =
            serde_json::from_slice(&fs::read(&manifest).with_context(|| {
                format!(
                    "读取 {}；先运行 scripts/fetch_external.py",
                    manifest.display()
                )
            })?)
            .context("解析外部来源清单")?;
        let mut files = BTreeMap::new();
        for snapshot in &snapshots {
            if snapshot.path.components().count() != 1
                || !matches!(
                    snapshot.path.components().next(),
                    Some(std::path::Component::Normal(_))
                )
            {
                bail!("来源文件必须是目录内直接文件：{}", snapshot.path.display());
            }
            let path = directory.join(&snapshot.path);
            let bytes = fs::read(&path).with_context(|| format!("读取来源 {}", path.display()))?;
            if format!("{:x}", Sha256::digest(&bytes)) != snapshot.sha256 {
                bail!("来源 SHA256 不符：{}，停止验证", path.display());
            }
            if files
                .insert(
                    snapshot.id,
                    String::from_utf8(bytes)
                        .with_context(|| format!("来源不是 UTF-8：{}", path.display()))?,
                )
                .is_some()
            {
                bail!("来源清单重复 id：{:?}", snapshot.id);
            }
        }
        let get = |id: SourceId| {
            files
                .get(&id)
                .with_context(|| format!("来源清单缺少 {id:?}"))
        };
        let mut data = Self {
            snapshots,
            readings: BTreeMap::new(),
            orders: BTreeMap::new(),
            hanzi: BTreeMap::new(),
            roots: BTreeMap::new(),
            root_signatures: vec![],
            variants: BTreeMap::new(),
        };
        data.load_unihan(get(SourceId::Unihan)?)?;
        data.load_variants(get(SourceId::UnihanVariants)?)?;
        data.load_cedict(get(SourceId::Cedict)?)?;
        data.load_cnchar(get(SourceId::CncharSpell)?)?;
        for (index, line) in get(SourceId::Hanzi)?.lines().enumerate() {
            let row: Hanzi =
                serde_json::from_str(line).with_context(|| format!("hanzi.jsonl:{}", index + 1))?;
            for reading in &row.pinyin {
                data.add(
                    row.character,
                    SourceId::Hanzi,
                    reading,
                    format!("hanzi.jsonl:{}", index + 1),
                );
            }
            if data.hanzi.insert(row.character, (row, index + 1)).is_some() {
                bail!("Hanzi 重复字：行 {}", index + 1);
            }
        }
        let strokes: BTreeMap<char, Stroke> = serde_json::from_str(get(SourceId::CncharStrokes)?)?;
        for id in [SourceId::CncharOrder, SourceId::CncharOrderTrad] {
            let orders: BTreeMap<char, String> = serde_json::from_str(get(id)?)?;
            for (ch, ids) in orders {
                let five_strokes = ids
                    .chars()
                    .map(|id| strokes.get(&id).and_then(stroke_key))
                    .collect::<Option<String>>();
                let names = ids
                    .chars()
                    .map(|id| {
                        strokes
                            .get(&id)
                            .map_or_else(|| format!("unknown:{id}"), |s| s.name.clone())
                    })
                    .collect();
                data.orders.entry(ch).or_default().push(StrokeOrder {
                    source: id,
                    ids,
                    five_strokes,
                    names,
                });
            }
        }
        for line in get(SourceId::KeytaoRoots)?.lines().skip(1) {
            let (root, code) = line
                .trim()
                .split_once(',')
                .context("无效 keytao-roots CSV 行")?;
            let ch = one_char(root).context("字根不是一个 Unicode 字符")?;
            if code.is_empty() || code.len() > 2 || !code.chars().all(is_shape) {
                bail!("无效字根编码 {line}");
            }
            data.roots.insert(ch, code.into());
        }
        // Documented variants, not local dictionary-derived aliases.
        for (variant, root) in [
            ('氵', '水'),
            ('氺', '水'),
            ('亻', '人'),
            ('扌', '手'),
            ('龵', '手'),
            ('朩', '木'),
            ('士', '土'),
            ('钅', '金'),
            ('曰', '日'),
            ('⺼', '月'),
        ] {
            if let Some(code) = data.roots.get(&root).cloned() {
                data.roots.insert(variant, code);
            }
        }
        for values in data.readings.values_mut() {
            values.sort_by(|a, b| {
                (&a.source, &a.normalized, &a.locator, &a.raw).cmp(&(
                    &b.source,
                    &b.normalized,
                    &b.locator,
                    &b.raw,
                ))
            });
            values
                .dedup_by(|a, b| a.source == b.source && a.raw == b.raw && a.locator == b.locator);
        }
        data.derive_root_signatures();
        Ok(data)
    }

    fn derive_root_signatures(&mut self) {
        let mut signatures = BTreeSet::new();
        for root in self.roots.keys() {
            for order in self.orders.get(root).into_iter().flatten() {
                if let Some(strokes) = &order.five_strokes {
                    signatures.insert(RootSignature {
                        root: *root,
                        five_strokes: strokes.clone(),
                        source_character: *root,
                        order_source: order.source,
                        ids_source_line: None,
                    });
                }
            }
        }
        for (ch, (hanzi, line)) in &self.hanzi {
            let Some(leaves) = ids_leaves(&hanzi.decomposition) else {
                continue;
            };
            for order in self.orders.get(ch).into_iter().flatten() {
                let Some(strokes) = &order.five_strokes else {
                    continue;
                };
                if strokes.len() != hanzi.matches.len() {
                    continue;
                }
                for (path, root) in &leaves {
                    if !self.roots.contains_key(root) {
                        continue;
                    }
                    let indices: Vec<_> = hanzi
                        .matches
                        .iter()
                        .enumerate()
                        .filter_map(|(i, m)| (m.as_ref() == Some(path)).then_some(i))
                        .collect();
                    if indices.is_empty() || !indices.windows(2).all(|w| w[1] == w[0] + 1) {
                        continue;
                    }
                    let signature = strokes[indices[0]..=indices[indices.len() - 1]].to_owned();
                    signatures.insert(RootSignature {
                        root: *root,
                        five_strokes: signature,
                        source_character: *ch,
                        order_source: order.source,
                        ids_source_line: Some(*line),
                    });
                }
            }
        }
        // Keep a deterministic provenance example for each distinct signature.
        let mut seen = BTreeSet::new();
        self.root_signatures = signatures
            .into_iter()
            .filter(|s| seen.insert((s.root, s.five_strokes.clone())))
            .collect();
    }
    fn add(&mut self, ch: char, source: SourceId, raw: &str, locator: String) {
        self.readings.entry(ch).or_default().push(Reading {
            source,
            raw: raw.into(),
            normalized: normalize_pinyin(raw),
            phonetic_codes: phonetic_codes(raw),
            locator,
        });
    }

    fn load_variants(&mut self, raw: &str) -> Result<()> {
        for (index, line) in raw.lines().enumerate() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 3 {
                bail!("Unihan variants 行 {} 不是三个字段", index + 1);
            }
            let kind = match fields[1] {
                "kTraditionalVariant" => VariantKind::Traditional,
                "kSimplifiedVariant" => VariantKind::Simplified,
                "kZVariant" => VariantKind::ZVariant,
                _ => continue,
            };
            let ch = unicode_point(fields[0])?;
            for value in fields[2].split_whitespace() {
                let variant =
                    unicode_point(value.split('<').next().context("异体字关系缺少码位")?)?;
                self.variants.entry(ch).or_default().push(VariantLink {
                    character: variant,
                    kind,
                    locator: format!(
                        "unihan-variants.txt:{} {} {}",
                        index + 1,
                        fields[1],
                        fields[2]
                    ),
                });
            }
        }
        Ok(())
    }
    fn load_unihan(&mut self, raw: &str) -> Result<()> {
        for (index, line) in raw.lines().enumerate() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 3 {
                bail!("Unihan 行 {} 不是三个字段", index + 1);
            }
            let field = fields[1];
            if !["kMandarin", "kHanyuPinyin", "kXHC1983", "kHanyuPinlu"].contains(&field) {
                continue;
            }
            let point =
                u32::from_str_radix(fields[0].strip_prefix("U+").context("Unihan 缺少 U+")?, 16)?;
            let ch = char::from_u32(point).context("Unihan 非法码位")?;
            for section in fields[2].split_whitespace() {
                let readings = if field == "kMandarin" {
                    section
                } else if field == "kHanyuPinlu" {
                    section.split('(').next().unwrap_or("")
                } else {
                    section
                        .rsplit_once(':')
                        .context("Unihan 读音缺少位置分隔符")?
                        .1
                };
                for reading in readings.split(',').filter(|r| !r.is_empty()) {
                    self.add(
                        ch,
                        SourceId::Unihan,
                        reading,
                        format!("unihan-readings.txt:{} {field} {}", index + 1, fields[2]),
                    );
                }
            }
        }
        Ok(())
    }
    fn load_cedict(&mut self, raw: &str) -> Result<()> {
        for (index, line) in raw.lines().enumerate() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let (head, tail) = line
                .split_once('[')
                .with_context(|| format!("CEDICT 行 {} 缺少 [", index + 1))?;
            let (reading, _) = tail.split_once(']').context("CEDICT 缺少 ]")?;
            if reading.split_whitespace().count() != 1 {
                continue;
            }
            let headwords: Vec<_> = head.split_whitespace().collect();
            if headwords.len() != 2 {
                bail!("CEDICT 行 {} 不是两个词头", index + 1);
            }
            for head in headwords.into_iter().collect::<BTreeSet<_>>() {
                if let Some(ch) = one_char(head).filter(|c| is_han(*c)) {
                    self.add(
                        ch,
                        SourceId::Cedict,
                        reading,
                        format!("cedict.txt:{}", index + 1),
                    );
                }
            }
        }
        Ok(())
    }
    fn load_cnchar(&mut self, raw: &str) -> Result<()> {
        let dictionary: BTreeMap<String, String> = serde_json::from_str(raw)?;
        for (pinyin, compressed) in dictionary {
            let Some((position, body)) = compressed.split_once(':') else {
                bail!("cnchar 拼音 {pinyin} 缺少压缩头部");
            };
            position.parse::<usize>().context("cnchar 无效声调位置")?;
            let mut chars = body.chars();
            while let Some(ch) = chars.next() {
                let tone = chars
                    .next()
                    .and_then(|c| c.to_digit(10))
                    .context("cnchar 字符后缺少声调数字")?;
                let tone = if tone > 4 { tone - 5 } else { tone };
                self.add(
                    ch,
                    SourceId::CncharSpell,
                    &format!("{pinyin}{tone}"),
                    format!("cnchar-spell.json key={pinyin}"),
                );
            }
        }
        Ok(())
    }
    fn shape_evidence(&self, ch: char) -> ShapeEvidence {
        let orders = self.orders.get(&ch).cloned().unwrap_or_default();
        let hanzi = self.hanzi.get(&ch);
        let counts: BTreeSet<_> = orders
            .iter()
            .map(|o| o.ids.chars().count())
            .chain(hanzi.map(|(h, _)| h.matches.len()))
            .collect();
        ShapeEvidence {
            stroke_orders: orders,
            ids: hanzi.map(|(h, _)| h.decomposition.clone()),
            ids_source_line: hanzi.map(|(_, line)| *line),
            matched_strokes: hanzi.map(|(h, _)| h.matches.len()),
            unknown_stroke_matches: hanzi
                .map(|(h, _)| h.matches.iter().filter(|m| m.is_none()).count()),
            stroke_count_disagreement: counts.len() > 1,
        }
    }
    fn shape(&self, ch: char, entry: &Entry, p: Profile) -> ShapeCheck {
        let shape = match p {
            Profile::Standard | Profile::Supplement
                if entry.code.len() >= 2 && entry.code.chars().take(2).all(is_sound) =>
            {
                &entry.code[2..]
            }
            Profile::CssSingle
                if !entry.code.is_empty() && entry.code.chars().take(1).all(is_sound) =>
            {
                &entry.code[1..]
            }
            _ => {
                return ShapeCheck {
                    status: ShapeStatus::SpecialEncoding,
                    expected: vec![],
                    reason: "不是普通单字音形结构，未按整字首形码检查".into(),
                };
            }
        };
        if shape.is_empty() {
            return ShapeCheck {
                status: ShapeStatus::NoShape,
                expected: vec![],
                reason: "仅有音码，无法验证形码".into(),
            };
        }
        if !shape.chars().all(is_shape) {
            return ShapeCheck {
                status: ShapeStatus::SpecialEncoding,
                expected: vec![],
                reason: "形码区含非形码键，交由结构检查器检查".into(),
            };
        }
        if let Some(root) = self.roots.get(&ch) {
            let expected = format!("{root}{}", root.chars().last().unwrap());
            return ShapeCheck {
                status: if expected.starts_with(shape) {
                    ShapeStatus::RootPrefixSupported
                } else {
                    ShapeStatus::RootConflict
                },
                expected: vec![expected],
                reason: "依键道公开字根及字根本身重复末码规则，独立于本地单字码表".into(),
            };
        }
        let Some(orders) = self.orders.get(&ch) else {
            return ShapeCheck {
                status: ShapeStatus::MissingStrokeData,
                expected: vec![],
                reason: "cnchar 缺少该字笔顺；IDS 本身不足以确定键道二分取根".into(),
            };
        };
        let mut allowed = BTreeSet::new();
        for order in orders {
            let Some(strokes) = &order.five_strokes else {
                return ShapeCheck {
                    status: ShapeStatus::AmbiguousStrokeData,
                    expected: vec![],
                    reason: "笔形存在未解释标记，未据不完整笔顺判错".into(),
                };
            };
            if let Some(first) = strokes.chars().next() {
                allowed.insert(first.to_string());
            }
            // Conservative superset: a leading documented root is allowed if
            // its external stroke signature is a prefix. This does not decide
            // geometric crossings or whether the scheme should select it.
            for signature in &self.root_signatures {
                if !signature.five_strokes.is_empty()
                    && strokes.starts_with(&signature.five_strokes)
                {
                    allowed.insert(self.roots[&signature.root][..1].to_owned());
                }
            }
        }
        ShapeCheck{status:if allowed.contains(&shape[..1]){ShapeStatus::FirstShapeCompatible}else{ShapeStatus::FirstShapeReview},expected:allowed.into_iter().collect(),reason:"仅对照外部笔顺及可前置字根的保守首码集合；未验证后续形码、二分优先级、交错、地区字形差异".into()}
    }
}

fn one_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}
fn unicode_point(raw: &str) -> Result<char> {
    char::from_u32(u32::from_str_radix(
        raw.strip_prefix("U+").context("缺少 U+ 码位前缀")?,
        16,
    )?)
    .context("非法 Unicode 码位")
}
fn stroke_key(stroke: &Stroke) -> Option<char> {
    match stroke.name.as_str() {
        "横" | "提" => Some('v'),
        "竖" | "竖钩" => Some('i'),
        "撇" => Some('u'),
        "捺" | "点" => Some('o'),
        "点2" => None,
        _ if stroke.kind == "折笔" => Some('a'),
        _ => None,
    }
}

fn ids_leaves(ids: &str) -> Option<BTreeMap<Vec<usize>, char>> {
    fn parse(
        chars: &mut std::str::Chars<'_>,
        path: &mut Vec<usize>,
        leaves: &mut BTreeMap<Vec<usize>, char>,
    ) -> Option<()> {
        let c = chars.next()?;
        if matches!(c, '\u{2ff0}'..='\u{2ffb}') {
            let arity = if matches!(c, '⿲' | '⿳') { 3 } else { 2 };
            for i in 0..arity {
                path.push(i);
                parse(chars, path, leaves)?;
                path.pop();
            }
        } else if c != '？' {
            leaves.insert(path.clone(), c);
        }
        Some(())
    }
    let mut chars = ids.chars();
    let mut leaves = BTreeMap::new();
    parse(&mut chars, &mut vec![], &mut leaves)?;
    chars.next().is_none().then_some(leaves)
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SoundStatus {
    Supported,
    VariantReadingReview,
    ExternalConflict,
    UnmappedReading,
    MissingReading,
    SpecialEncoding,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ShapeStatus {
    RootPrefixSupported,
    RootConflict,
    FirstShapeCompatible,
    FirstShapeReview,
    NoShape,
    SpecialEncoding,
    MissingStrokeData,
    AmbiguousStrokeData,
}
#[derive(Clone, Debug, Serialize)]
pub struct ShapeCheck {
    pub status: ShapeStatus,
    pub expected: Vec<String>,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Location {
    pub file: PathBuf,
    pub line: usize,
    pub active: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct CodeCheck {
    pub code: String,
    pub profile: Profile,
    pub locations: Vec<Location>,
    pub sound_status: SoundStatus,
    pub supporting_sources: BTreeSet<SourceId>,
    pub sources_with_readings: BTreeSet<SourceId>,
    pub variant_support: Vec<VariantSupport>,
    pub unihan_and_cedict_both_conflict: bool,
    pub shape: ShapeCheck,
}
#[derive(Debug, Serialize)]
pub struct CharacterCheck {
    pub character: char,
    pub readings: Vec<Reading>,
    pub expected_phonetics: BTreeSet<String>,
    pub shape_evidence: ShapeEvidence,
    pub variants: Vec<VariantLink>,
    pub codes: Vec<CodeCheck>,
}
#[derive(Debug, Serialize)]
pub struct Verification {
    pub root: PathBuf,
    pub all: bool,
    pub snapshots: Vec<Snapshot>,
    pub root_signatures: Vec<RootSignature>,
    pub characters: Vec<CharacterCheck>,
    pub local_entry_count: usize,
    pub sound_counts: BTreeMap<SoundStatus, usize>,
    pub shape_counts: BTreeMap<ShapeStatus, usize>,
    pub limitations: Vec<String>,
}

pub fn verify(ws: &Workspace, data: &ExternalData, all: bool) -> Verification {
    let mut grouped: BTreeMap<char, BTreeMap<(String, Profile), Vec<&Entry>>> = BTreeMap::new();
    let mut total = 0;
    for e in ws
        .entries
        .iter()
        .filter(|e| ws.selected(e, all) && profile(&e.file) != Profile::Foreign)
    {
        if let Some(ch) = one_char(&e.text).filter(|ch| is_han(*ch)) {
            grouped
                .entry(ch)
                .or_default()
                .entry((e.code.clone(), profile(&e.file)))
                .or_default()
                .push(e);
            total += 1;
        }
    }
    let mut report=Verification{root:ws.root.clone(),all,snapshots:data.snapshots.clone(),root_signatures:data.root_signatures.clone(),characters:vec![],local_entry_count:total,sound_counts:BTreeMap::new(),shape_counts:BTreeMap::new(),limitations:vec![
        "外部数据未使用本地音形码补全。声调不参与键道音码，无法验证声调；多音和公开飞键均接受。".into(),
        "Make Me a Hanzi 派生自 Unihan/CJKlib，不与 Unihan 当作独立票；cnchar 来源网络且独立性未确定。Unihan 多字段也只算一个渠道。".into(),
        "ExternalConflict 表示现有外部读音均不支持该码，不等于穷尽历史、姓氏、方言及生僻读音；不自动改词库。两渠道冲突专指 Unihan 与 CC-CEDICT 都有可映射读音且均不支持。".into(),
        "若有无法映射的外部读音，保守记 UnmappedReading；只支持一种来源、缺漏或来源分歧分别保留证据。字符不做简繁、异体或 NFKC 合并；原字无支持但相关异体读音支持时记 VariantReadingReview，不判为原字已验证。".into(),
        "形码仅对公开字根本身验证规则前缀；其他字只检查外部笔顺和可前置字根的保守首码集合。FirstShapeCompatible 不代表全形码正确，FirstShapeReview 需要人工检查笔顺及地区字形。".into(),
        "Make Me a Hanzi 的 IDS/matches 与 cnchar 笔顺保留用于核对；未将通用 IDS 当作键道二分，也未用笔画总数相同推断笔顺一致。".into(),
    ]};
    for (ch, entries) in grouped {
        let readings = data.readings.get(&ch).cloned().unwrap_or_default();
        let expected_phonetics = readings
            .iter()
            .flat_map(|r| r.phonetic_codes.iter().cloned())
            .collect();
        let mut checks = Vec::new();
        for ((code, p), locations) in entries {
            let length = if p == Profile::CssSingle
                && !code.is_empty()
                && code.chars().take(1).all(is_sound)
            {
                Some(1)
            } else if [Profile::Standard, Profile::Supplement, Profile::Syxb].contains(&p)
                && code.chars().take(code.len().min(2)).all(is_sound)
                && !code.is_empty()
            {
                Some(code.len().min(2))
            } else {
                None
            };
            let sources_with_readings = readings.iter().map(|r| r.source).collect();
            let supporting_sources: BTreeSet<_> = readings
                .iter()
                .filter(|r| {
                    length
                        .is_some_and(|n| r.phonetic_codes.iter().any(|c| c.starts_with(&code[..n])))
                })
                .map(|r| r.source)
                .collect();
            let unmapped = readings.iter().any(|r| r.phonetic_codes.is_empty());
            let variant_support: Vec<_> = data
                .variants
                .get(&ch)
                .into_iter()
                .flatten()
                .flat_map(|link| {
                    data.readings
                        .get(&link.character)
                        .into_iter()
                        .flatten()
                        .filter(|r| {
                            length.is_some_and(|n| {
                                r.phonetic_codes.iter().any(|p| p.starts_with(&code[..n]))
                            })
                        })
                        .map(|r| VariantSupport {
                            variant: link.clone(),
                            reading: r.clone(),
                        })
                })
                .collect();
            let status = if length.is_none() {
                SoundStatus::SpecialEncoding
            } else if !supporting_sources.is_empty() {
                SoundStatus::Supported
            } else if !variant_support.is_empty() {
                SoundStatus::VariantReadingReview
            } else if readings.is_empty() {
                SoundStatus::MissingReading
            } else if unmapped {
                SoundStatus::UnmappedReading
            } else {
                SoundStatus::ExternalConflict
            };
            let both_conflict = status == SoundStatus::ExternalConflict
                && [SourceId::Unihan, SourceId::Cedict].iter().all(|id| {
                    readings
                        .iter()
                        .any(|r| r.source == *id && !r.phonetic_codes.is_empty())
                });
            let shape = data.shape(ch, locations[0], p);
            *report.sound_counts.entry(status).or_default() += 1;
            *report.shape_counts.entry(shape.status).or_default() += 1;
            checks.push(CodeCheck {
                code,
                profile: p,
                locations: locations
                    .into_iter()
                    .map(|e| Location {
                        file: e.file.clone(),
                        line: e.line,
                        active: ws.active_files.contains(&e.file),
                    })
                    .collect(),
                sound_status: status,
                supporting_sources,
                sources_with_readings,
                variant_support,
                unihan_and_cedict_both_conflict: both_conflict,
                shape,
            });
        }
        report.characters.push(CharacterCheck {
            character: ch,
            readings,
            expected_phonetics,
            shape_evidence: data.shape_evidence(ch),
            variants: data.variants.get(&ch).cloned().unwrap_or_default(),
            codes: checks,
        });
    }
    report
}

impl Verification {
    pub fn markdown(&self) -> String {
        let mut out = format!(
            "# 外部数据交叉验证单字编码\n\n{} 个汉字，{} 条本地单字记录。统计按 (字、编码、规则类型) 去重，来源位置完整保留在 JSON。\n\n## 音码覆盖\n\n|状态|编码项|\n|---|---:|\n",
            self.characters.len(),
            self.local_entry_count
        );
        for (status, n) in &self.sound_counts {
            out.push_str(&format!("|{status:?}|{n}|\n"));
        }
        out.push_str("\n## 形码覆盖\n\n|状态|编码项|\n|---|---:|\n");
        for (s, n) in &self.shape_counts {
            out.push_str(&format!("|{s:?}|{n}|\n"));
        }
        out.push_str("\n## 来源\n\n");
        for s in &self.snapshots {
            out.push_str(&format!(
                "- {:?}：{}；修订 {}；{}；SHA256 `{}`。{}\n",
                s.id, s.url, s.revision, s.license, s.sha256, s.lineage
            ));
        }
        out.push_str("\n## 证据边界\n\n");
        for n in &self.limitations {
            out.push_str(&format!("- {n}\n"));
        }
        out.push_str("\n## 音码待核对\n\n|字|实际编码|原字外部音码集合|原字拼音（按来源）|异体读音支持|状态|Unihan/CEDICT均冲突|位置|\n|---|---|---|---|---|---|---|---|\n");
        for ch in &self.characters {
            for c in &ch.codes {
                if matches!(
                    c.sound_status,
                    SoundStatus::ExternalConflict
                        | SoundStatus::VariantReadingReview
                        | SoundStatus::UnmappedReading
                        | SoundStatus::MissingReading
                ) {
                    out.push_str(&format!(
                        "|{}|{}|{}|{}|{}|{:?}|{}|{}|\n",
                        ch.character,
                        c.code,
                        ch.expected_phonetics
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(","),
                        reading_summary(&ch.readings),
                        c.variant_support
                            .iter()
                            .map(|v| format!("{}:{}", v.variant.character, v.reading.raw))
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect::<Vec<_>>()
                            .join(","),
                        c.sound_status,
                        c.unihan_and_cedict_both_conflict,
                        locations_text(&c.locations)
                    ));
                }
            }
        }
        out.push_str("\n## 形码待核对\n\n仅列字根冲突与首形码疑点；其余字的后续形码尚未验证。\n\n|字|编码|预期字根全形/保守首码|状态|外部笔顺及IDS|位置|\n|---|---|---|---|---|---|\n");
        for ch in &self.characters {
            for c in &ch.codes {
                if matches!(
                    c.shape.status,
                    ShapeStatus::RootConflict | ShapeStatus::FirstShapeReview
                ) {
                    out.push_str(&format!(
                        "|{}|{}|{}|{:?}|{} / {}|{}|\n",
                        ch.character,
                        c.code,
                        c.shape.expected.join(","),
                        c.shape.status,
                        ch.shape_evidence
                            .stroke_orders
                            .iter()
                            .filter_map(|o| o.five_strokes.as_deref())
                            .collect::<Vec<_>>()
                            .join(","),
                        ch.shape_evidence.ids.as_deref().unwrap_or("缺失"),
                        locations_text(&c.locations)
                    ));
                }
            }
        }
        out
    }
}
fn locations_text(locations: &[Location]) -> String {
    locations
        .iter()
        .map(|l| {
            format!(
                "{}:{}{}",
                l.file.file_name().unwrap_or_default().to_string_lossy(),
                l.line,
                if l.active { "（启用）" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn reading_summary(readings: &[Reading]) -> String {
    let mut groups: BTreeMap<SourceId, BTreeSet<&str>> = BTreeMap::new();
    for r in readings {
        groups.entry(r.source).or_default().insert(&r.raw);
    }
    groups
        .into_iter()
        .map(|(id, values)| {
            format!(
                "{id:?}:{}",
                values.into_iter().collect::<Vec<_>>().join(",")
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Workspace, ExternalData) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("xkjd6.schema.yaml"),
            "translator:\n  dictionary: xkjd6.extended\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("xkjd6.extended.dict.yaml"),
            "---\nname: xkjd6.extended\nimport_tables: [user.xkjd6]\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("user.xkjd6.dict.yaml"),
            "---\nname: user.xkjd6\n...\n字\tzz\n人\trnii\n扎\tfsiu\n",
        )
        .unwrap();
        let raw = [
            (
                SourceId::Unihan,
                "U+5B57\tkMandarin\tzì\nU+4EBA\tkHanyuPinyin\t1000.000:rén\nU+624E\tkXHC1983\t1457.010:zhā\n",
            ),
            (SourceId::UnihanVariants, ""),
            (
                SourceId::Cedict,
                "字 字 [zi4] /character/\n人 人 [ren2] /person/\n扎 扎 [zha1] /prick/\n",
            ),
            (
                SourceId::CncharSpell,
                r#"{"zi":"0:字4","ren":"1:人2","zha":"1:扎1"}"#,
            ),
            (SourceId::CncharOrder, r#"{"人":"sl","扎":"jgjt"}"#),
            (SourceId::CncharOrderTrad, "{}"),
            (
                SourceId::CncharStrokes,
                r#"{"s":{"name":"撇","type":"平笔"},"l":{"name":"捺","type":"平笔"},"j":{"name":"横","type":"平笔"},"g":{"name":"竖钩","type":"折笔"},"t":{"name":"竖弯钩","type":"折笔"}}"#,
            ),
            (
                SourceId::Hanzi,
                "{\"character\":\"扎\",\"pinyin\":[\"zhā\"],\"decomposition\":\"⿰扌乚\",\"matches\":[[0],[0],[0],[1]]}\n",
            ),
            (SourceId::KeytaoRoots, "root,code\n人,i\n手,iu\n"),
        ];
        let sources = dir.path().join("external");
        fs::create_dir(&sources).unwrap();
        let mut manifest = Vec::new();
        for (i, (id, text)) in raw.iter().enumerate() {
            let path = PathBuf::from(format!("{i}.txt"));
            fs::write(sources.join(&path), text).unwrap();
            manifest.push(Snapshot {
                id: *id,
                url: "fixture".into(),
                path,
                revision: "fixture".into(),
                license: "fixture".into(),
                lineage: "fixture".into(),
                sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
                download_sha256: "fixture".into(),
            });
        }
        fs::write(
            sources.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let ws = Workspace::load(dir.path(), "xkjd6").unwrap();
        let data = ExternalData::load(&sources).unwrap();
        (dir, ws, data)
    }
    #[test]
    fn loaders_keep_source_evidence_and_consensus_without_local_reference() {
        let (_dir, ws, data) = fixture();
        let result = verify(&ws, &data, true);
        let word = result
            .characters
            .iter()
            .find(|c| c.character == '字')
            .unwrap();
        assert_eq!(word.codes[0].sound_status, SoundStatus::ExternalConflict);
        assert!(word.codes[0].unihan_and_cedict_both_conflict);
        assert_eq!(word.expected_phonetics, BTreeSet::from(["zk".into()]));
        assert!(
            word.readings
                .iter()
                .any(|r| r.source == SourceId::CncharSpell && r.raw == "zi4")
        );
        let ren = result
            .characters
            .iter()
            .find(|c| c.character == '人')
            .unwrap();
        assert_eq!(ren.codes[0].shape.status, ShapeStatus::RootPrefixSupported);
        assert!(ren.codes[0].supporting_sources.contains(&SourceId::Unihan));
    }
    #[test]
    fn missing_radical_stroke_order_is_recovered_from_external_matches() {
        let (_dir, ws, data) = fixture();
        assert!(!data.orders.contains_key(&'扌'));
        assert!(
            data.root_signatures
                .iter()
                .any(|s| s.root == '扌' && s.five_strokes == "viv" && s.ids_source_line == Some(1))
        );
        let result = verify(&ws, &data, true);
        assert_eq!(
            result
                .characters
                .iter()
                .find(|c| c.character == '扎')
                .unwrap()
                .codes[0]
                .shape
                .status,
            ShapeStatus::FirstShapeCompatible
        );
    }
    #[test]
    fn snapshots_are_verified_before_processing() {
        let (dir, _ws, _data) = fixture();
        fs::write(dir.path().join("external/0.txt"), "tampered").unwrap();
        assert!(
            ExternalData::load(&dir.path().join("external"))
                .unwrap_err()
                .to_string()
                .contains("SHA256")
        );
    }
    #[test]
    fn unknown_readings_are_not_silently_discarded_and_no_variant_merging() {
        let (_dir, ws, mut data) = fixture();
        data.add('字', SourceId::Unihan, "m̄", "fixture".into());
        let result = verify(&ws, &data, true);
        assert_eq!(
            result
                .characters
                .iter()
                .find(|c| c.character == '字')
                .unwrap()
                .codes[0]
                .sound_status,
            SoundStatus::UnmappedReading
        );
        data.add('還', SourceId::Unihan, "huán", "fixture".into());
        assert!(!data.readings.contains_key(&'还'));
        assert!(ids_leaves("⿰扌？").unwrap().values().any(|c| *c == '扌'));
        assert!(ids_leaves("⿰扌").is_none());
    }

    #[test]
    fn variant_support_is_separate_from_literal_character_support() {
        let (dir, _, mut data) = fixture();
        let path = dir.path().join("user.xkjd6.dict.yaml");
        let mut raw = fs::read_to_string(&path).unwrap();
        raw.push_str("许\thj\n");
        fs::write(path, raw).unwrap();
        let ws = Workspace::load(dir.path(), "xkjd6").unwrap();
        data.add('许', SourceId::Unihan, "xǔ", "fixture".into());
        data.add('許', SourceId::Unihan, "hǔ", "fixture".into());
        data.load_variants("U+8BB8\tkTraditionalVariant\tU+8A31\n")
            .unwrap();
        let report = verify(&ws, &data, true);
        let check = &report
            .characters
            .iter()
            .find(|c| c.character == '许')
            .unwrap()
            .codes[0];
        assert_eq!(check.sound_status, SoundStatus::VariantReadingReview);
        assert!(check.supporting_sources.is_empty());
        assert_eq!(check.variant_support[0].variant.character, '許');
        assert_eq!(data.readings[&'许'].len(), 1);
    }
}
