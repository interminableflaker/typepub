use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use anyhow::Context as _;
use roxmltree::Node;
use simplecss::StyleSheet;
use url::Url;

pub fn ebook_directory() -> anyhow::Result<std::path::PathBuf> {
    #[cfg(windows)]
    let mut dir = dirs::document_dir().context("could not locate home directory")?;
    #[cfg(not(windows))]
    let mut dir = dirs::home_dir().context("could not locate home directory")?;
    dir.push("books");
    Ok(dir)
    // fs::read_dir(dir).map_err(Into::into)
}

struct EpubArchive {
    archive: zip::ZipArchive<io::BufReader<fs::File>>,
    manifest: Manifest,
    root: PathBuf,
}

pub struct Epub {
    archive: EpubArchive,
    metadata: Metadata,
    spine: Spine,
    toc: Toc,
}

impl Epub {
    pub fn from_path(path: &Path) -> anyhow::Result<Self> {
        EpubPreview::from_file(path)?.full()
    }

    pub fn name(&self) -> &str {
        &self.metadata.title
    }

    pub fn author(&self) -> Option<&Author> {
        self.metadata.creators.first()
    }
}

#[derive(Debug, Clone)]
struct Item {
    name: String,
    path: String,
    mime: String,
}

#[derive(Debug)]
struct Manifest(Vec<Item>);

impl Manifest {
    fn parse(node: Node) -> anyhow::Result<(Self, Option<usize>)> {
        let mut items = vec![];
        let mut toc = None;
        for child in node.children().filter(Node::is_element) {
            let name = child
                .attribute("id")
                .map(ToOwned::to_owned)
                .context("manifest item missing id")?;
            let href = child
                .attribute("href")
                .context("manifest item missing href")?;
            let path = String::from(href);
            let mime = child
                .attribute("media-type")
                .map(ToOwned::to_owned)
                .context("manifest item missing mime")?;

            if matches!(child.attribute("properties"), Some("nav")) {
                toc = Some(items.len());
            }

            items.push(Item { name, path, mime });
        }

        Ok((Self(items), toc))
    }

    // fn item(&self, path: &str) -> Option<&Item> {
    //     self.0.iter().find(|item| item.path == path)
    // }

    fn item_idx(&self, path: &str) -> Option<usize> {
        self.0.iter().position(|item| item.path == path)
    }

    fn item_idx_by_name(&self, name: &str) -> Option<usize> {
        self.0.iter().position(|item| item.name == name)
    }
}

#[derive(Debug)]
struct Spine(Vec<usize>);

impl Spine {
    fn parse(archive: &EpubArchive, node: Node) -> anyhow::Result<(Self, Option<usize>)> {
        let ncx = node
            .attribute("toc")
            .and_then(|name| archive.manifest.item_idx_by_name(name));
        let spine = Self(
            node.children()
                .filter_map(|node| node.attribute("idref"))
                .map(|name| archive.manifest.item_idx_by_name(name))
                .collect::<Option<Vec<usize>>>()
                .context("invalid spine")?,
        );
        Ok((spine, ncx))
    }

    fn manifest_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().copied()
    }
}

#[derive(Debug)]
struct Toc(Vec<TocEntry>);

#[derive(Debug)]
struct TocEntry {
    name: String,
    fragment: Option<String>,
    idx: usize,
}

impl Toc {
    fn parse_v3(archive: &mut EpubArchive, spine: &Spine, toc_idx: usize) -> anyhow::Result<Self> {
        fn is_nav(n: &Node) -> bool {
            n.tag_name().name() == "nav"
                && matches!(
                    n.attribute(("http://www.idpf.org/2007/ops", "type")),
                    Some("toc")
                )
        }

        fn find_nav<'a, 'input>(node: Node<'a, 'input>) -> Option<Node<'a, 'input>> {
            for child in node.children() {
                if is_nav(&child) {
                    return Some(child);
                }
                if let Some(nav) = find_nav(child) {
                    return Some(nav);
                }
            }
            None
        }

        // let toc_item = &manifest.0[toc_idx];
        let data = archive.retrieve(toc_idx)?;
        let xml = roxmltree::Document::parse(&data)?;
        let mut elements = xml.root_element().children().filter(Node::is_element);
        let _head = elements.next().context("toc missing head")?;
        let body = elements.next().context("toc missing body")?;
        let toc_nav = find_nav(body).context("toc missing nav")?;

        let mut entries = Vec::new();
        let list = toc_nav
            .children()
            .filter(Node::is_element)
            .find(|n| n.tag_name().name() == "ol")
            .context("toc missing navlist")?;
        // println!("{:?}", list.document());
        let toc_uri = archive.item_uri(toc_idx)?;
        for item in list.children().filter(Node::is_element) {
            let element = item.first_element_child().context("invalid toc item")?;
            let href = element.attribute("href").context("toc item missing href")?;
            let fragment = href.rsplit_once('#').map(|(_, frag)| frag.to_owned());
            let manifest_idx = archive
                .items()
                .enumerate()
                .position(|(idx, _)| {
                    let item_uri = archive.item_uri(idx);
                    let href = toc_uri.join(href);
                    if let (Ok(item_uri), Ok(href)) = (item_uri, href) {
                        item_uri.path() == href.path()
                    } else {
                        false
                    }
                })
                .context("toc reference missing in manifest")?;
            let idx = spine
                .manifest_indices()
                .position(|i| i == manifest_idx)
                .context("toc reference missing in spine")?;
            let name = element.text().context("toc item missing name")?.to_owned();
            entries.push(TocEntry {
                name,
                fragment,
                idx,
            });
        }

        Ok(Toc(entries))
    }

    fn parse_v2(archive: &mut EpubArchive, spine: &Spine, ncx_idx: usize) -> anyhow::Result<Self> {
        let data = archive.retrieve(ncx_idx)?;
        let xml = roxmltree::Document::parse(&data).unwrap();

        let nav_map = xml
            .root_element()
            .children()
            .filter(Node::is_element)
            .find(|n| n.tag_name().name() == "navMap")
            .context("toc missing nav map")?;

        fn visit_navpoint(
            archive: &EpubArchive,
            spine: &Spine,
            entries: &mut Vec<TocEntry>,
            play_order: &mut Vec<usize>,
            nav_point: Node,
        ) -> anyhow::Result<()> {
            // let id = nav_point.attribute("id").unwrap();
            if let Some(idx) = nav_point.attribute("playOrder").map(str::parse).transpose()? {
                play_order.push(idx);
            }

            let mut elements = nav_point.children().filter(Node::is_element);
            let name = elements
                .next()
                .and_then(|e| e.first_element_child())
                .and_then(|e| e.text())
                .map(ToOwned::to_owned)
                .context("nav point is missing valid name")?;
            let content = elements
                .next()
                .and_then(|e| e.attribute("src"))
                .context("nav point is missing src attribute")?;
            // panic!(
            //     "{}",
            //     archive.parse_hyperlink(dbg!(&archive.manifest.0[ncx].path), content)?
            // );

            let (path, fragment) = match content.rsplit_once('#') {
                Some((path, frag)) => (path.to_lowercase(), Some(frag).map(ToOwned::to_owned)),
                None => (content.to_lowercase(), None),
            };

            let idx = archive
                .items()
                .position(|item| item.path.to_lowercase() == path)
                .and_then(|idx| spine.manifest_indices().position(|i| i == idx))
                .unwrap_or(0);

            entries.push(TocEntry {
                name,
                fragment,
                idx,
            });

            for subpoint in elements {
                visit_navpoint(archive, spine, entries, play_order, subpoint)?;
            }

            Ok(())
        }

        let mut entries = Vec::new();
        let mut play_order = Vec::new();
        for nav_point in nav_map
            .children()
            .filter(Node::is_element)
            .skip_while(|n| n.tag_name().name() == "navInfo")
        {
            visit_navpoint(archive, spine, &mut entries, &mut play_order, nav_point)?;
        }
        if !play_order.is_empty() {
            assert_eq!(
                entries.len(), play_order.len(),
                "if one ncx entry has a play order attribute, they all should",
            );
            let mut zipped = play_order.into_iter().zip(entries).collect::<Vec<_>>();
            zipped.sort_by_key(|(play_order, _)| *play_order);
            entries = zipped.into_iter().map(|(_, e)| e).collect();
        }

        // panic!("{:#?}", entries);

        Ok(Self(entries))
    }
}

struct EpubPreview {
    archive: zip::ZipArchive<io::BufReader<std::fs::File>>,
    root: PathBuf,
    metadata: Metadata,
    version: u8,
    rootfile: String,
}

#[derive(Debug)]
struct Metadata {
    identifier: String,
    title: String,
    language: String,
    creators: Vec<Author>,
}

impl Metadata {
    fn parse(node: Node) -> anyhow::Result<Self> {
        let mut identifier = None;
        let mut title = None;
        let mut language = None;
        let mut creators = Vec::new();
        for child in node.children().filter(Node::is_element) {
            match child.tag_name().name() {
                "identifier" => identifier = child.text().map(ToOwned::to_owned),
                "title" => title = child.text().map(ToOwned::to_owned),
                "language" => language = child.text().map(ToOwned::to_owned),
                "creator" => {
                    if let Some(raw) = child
                        .attribute(("http://www.idpf.org/2007/opf", "file-as"))
                        .or_else(|| child.text())
                    {
                        for name in if raw.contains('&') {
                            raw.split("&")
                        } else {
                            raw.split(" and ")
                        } {
                            if let Some(author) = Author::parse(name) {
                                creators.push(author);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // if let Some(title) = &title {
        //     print!("{}", title);
        //     if let Some(creator) = creators.first() {
        //         print!(" by {}", creator);
        //     }
        //     println!();
        // }
        Ok(Metadata {
            identifier: identifier.context("missing identifier")?,
            title: title.context("missing title")?,
            language: language.context("missing language")?,
            creators,
        })
    }
}

impl EpubPreview {
    fn title(&self) -> &str {
        &self.metadata.title
    }

    fn creator(&self) -> Option<&Author> {
        self.metadata.creators.first()
    }

    fn from_file(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        use fs::File;
        use io::Read as _;

        // let ts = std::time::Instant::now();
        let fd = File::open(path)?;
        let mut archive = zip::ZipArchive::new(std::io::BufReader::new(fd))?;

        let mut buf = String::new();
        // println!("1: {:?}", ts.elapsed());

        archive
            .by_name("META-INF/container.xml")?
            .read_to_string(&mut buf)?;
        let container = roxmltree::Document::parse(&buf)?;
        // println!("2: {:?}", ts.elapsed());

        let rootfile_path = container
            .descendants()
            .find(|n| n.has_tag_name("rootfile"))
            .context("missing rootfile")
            .and_then(|rf| rf.attribute("full-path").context("rootfile missing path"))?;

        let root = {
            let path = std::path::Path::new(rootfile_path);
            anyhow::ensure!(path.is_relative(), "rootfile path not relative");
            path.parent().unwrap().to_path_buf()
        };

        let rootfile = {
            let mut rootfile = archive.by_name(rootfile_path)?;
            buf.clear();
            rootfile.read_to_string(&mut buf)?;
            roxmltree::Document::parse(&buf)?
        };

        let version = rootfile
            .root_element()
            .attribute("version")
            .context("rootfile missing version")?
            .as_bytes()[0]
            - b'0';
        anyhow::ensure!(version > 1, "unsupported epub version");

        let metadata = rootfile
            .root_element()
            .first_element_child()
            .context("rootfile missing metadata")
            .and_then(Metadata::parse)?;

        // println!("3: {:?}", ts.elapsed());

        Ok(Self {
            archive,
            root,
            version,
            metadata,
            rootfile: buf,
        })
    }

    fn full(self) -> anyhow::Result<Epub> {
        let Self {
            archive,
            root,
            version,
            metadata,
            rootfile,
        } = self;

        let rootfile = roxmltree::Document::parse(&rootfile)?;

        let mut children = rootfile
            .root_element()
            .children()
            .filter(Node::is_element)
            .skip(1);

        let (manifest, toc_idx) = children
            .next()
            .context("rootfile missing manifest")
            .and_then(Manifest::parse)?;

        let mut archive = EpubArchive {
            archive,
            manifest,
            root,
        };

        let (spine, ncx_idx) = children
            .next()
            .context("rootfile missing spine")
            .and_then(|child| Spine::parse(&archive, child))?;

        let toc = match version {
            2 => Toc::parse_v2(&mut archive, &spine, ncx_idx.context("missing ncx idx")?)?,
            3 => Toc::parse_v3(&mut archive, &spine, toc_idx.context("missing toc idx")?)?,
            _ => anyhow::bail!(
                "unsupported epub version: {} (supported versions are 2, 3)",
                version
            ),
        };

        Ok(Epub {
            archive,
            metadata,
            spine,
            toc,
        })
    }
}

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    pub struct Style: u8 {
        const ITALIC = 0b001;
        const BOLD   = 0b010;
        const CENTER = 0b100;
    }
}

pub enum Content<'a> {
    Title,
    Text(Style, &'a str),
    Linebreak,
    Image,
}

impl EpubArchive {
    fn name_in_archive(&self, path: &str) -> String {
        let mut abs_path = self.root.to_path_buf(); // is there no way to avoid this?
        abs_path.push(path);
        abs_path.into_os_string().into_string().unwrap()
    }

    fn retrieve(&mut self, item: usize) -> anyhow::Result<String> {
        let item = &self.manifest.0[item];
        let abs_path = self.name_in_archive(&item.path);
        let mut data = String::new();
        let mut file = self.archive.by_name(&abs_path)?;
        file.read_to_string(&mut data)?;
        Ok(data)
    }

    fn retrieve_idx(&mut self, item: usize) -> anyhow::Result<String> {
        let item = &self.manifest.0[item];
        let abs_path = self.name_in_archive(&item.path);
        let mut data = String::new();
        let mut file = self.archive.by_name(&abs_path)?;
        file.read_to_string(&mut data)?;
        Ok(data)
    }

    // fn uri_between_items(&self, from: usize, to: usize) -> anyhow::Result<Url> {
    //     let from = &self.manifest.0[from].path;
    //     let to = &self.manifest.0[to].path;
    //     Ok(Url::parse("epub:/")?.join(from)?.join(to)?)
    // }

    fn item_uri(&self, idx: usize) -> anyhow::Result<Url> {
        let path = &self.manifest.0[idx].path;
        Ok(Url::parse("epub:/")?.join(path)?)
    }

    fn resolve_hyperlink(&self, item: usize, href: &str) -> anyhow::Result<usize> {
        let item = &self.manifest.0[item];
        let url: Url = parse_hyperlink(&item.path, href)?;
        self.manifest
            .item_idx(&url.path()[1..])
            .context("broken epub href")
    }

    fn items(&self) -> impl Iterator<Item = &Item> {
        self.manifest.0.iter()
    }
}

struct XmlNode<'a, 'input: 'a>(Node<'a, 'input>);

impl simplecss::Element for XmlNode<'_, '_> {
    fn parent_element(&self) -> Option<Self> {
        self.0.parent_element().map(XmlNode)
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        self.0
            .prev_siblings()
            .find(|n| n.is_element())
            .map(XmlNode)
    }

    fn has_local_name(&self, local_name: &str) -> bool {
        self.0.tag_name().name() == local_name
    }

    fn attribute_matches(&self, local_name: &str, operator: simplecss::AttributeOperator) -> bool {
        self.0.attribute(local_name).map_or(false, |v| operator.matches(v))
    }

    fn pseudo_class_matches(&self, class: simplecss::PseudoClass) -> bool {
        match class {
            simplecss::PseudoClass::FirstChild => self.prev_sibling_element().is_none(),
            _ => false, // Since we are querying a static XML we can ignore other pseudo-classes.
        }
    }
}

impl Epub {
    pub fn traverse(
        &mut self,
        entry: usize,
        mut cb: impl FnMut(Content<'_>),
    ) -> anyhow::Result<(&str, &str)> {
        let mut fragment = self.toc.0[entry].fragment.as_deref();
        let start = self.toc.0[entry].idx;
        let (end, end_fragment) = match self.toc.0.get(entry + 1) {
            Some(entry) if entry.fragment.is_some() => (entry.idx + 1, entry.fragment.as_deref()),
            Some(entry) => (entry.idx, None),
            None => (self.toc.0.last().unwrap().idx + 1, None),
        };

        for (i, item_idx) in (start..end).map(|i| (i, self.spine.0[i])) {
            let mut data = self.archive.retrieve(item_idx)?;

            let xml = match roxmltree::Document::parse(&data) {
                Ok(x) => x,
                Err(roxmltree::Error::UnknownEntityReference(name, _)) => {
                    // TODO: may have to do this recursively, if there are more
                    //       entities looking to cause trouble
                    let (needle, replacement) = match name.as_ref() {
                        "nbsp" => ("&nbsp;", " "),
                        _ => panic!(),
                    };

                    data = data.replace(needle, replacement);
                    roxmltree::Document::parse(&data).unwrap()
                }
                Err(e) => panic!("{e}"),
            };

            let (head, body) = {
                let mut containers = xml
                    .root_element()
                    .children()
                    .filter(roxmltree::Node::is_element);
                (
                    containers.next().context("missing head")?,
                    containers.next().context("missing body")?,
                )
            };

            let mut raw_stylesheets = Vec::new();
            for node in head.children().filter(Node::is_element) {
                match node.tag_name().name() {
                    "link" if node.attribute("rel") == Some("stylesheet") => {
                        let href = node.attribute("href").unwrap();
                        let css_item = self.archive.resolve_hyperlink(item_idx, href)?;
                        let css = self.archive.retrieve_idx(css_item)?;
                        raw_stylesheets.push(css);
                    }
                    "style" if matches!(node.attribute("type"), Some("text/css") | None) => {
                        raw_stylesheets
                            .push(node.text().context("style tag without text")?.to_owned());
                    }
                    _ => {}
                }
            }

            let mut styles = simplecss::StyleSheet::new();
            for style in raw_stylesheets.iter() {
                styles.parse_more(style);
            }

            let mut rules = Vec::new();

            for (i, rule) in styles.rules.iter().enumerate() {
                for dec in &rule.declarations {
                    match dec.name {
                        "font-style" if dec.value == "italic" || dec.value.contains("oblique") => {
                            rules.push((i, Style::ITALIC))
                        }
                        "font-weight"
                            if matches!(dec.value, "bold" | "bolder")
                                || dec.value.parse::<usize>().is_ok_and(|x| x > 400) =>
                        {
                            rules.push((i, Style::BOLD))
                        }
                        "text-align" if dec.value == "center" => rules.push((i, Style::CENTER)),
                        _ => {}
                    }
                }
            }

            // panic!("{:#?}", body.document().input_text());

            let cutoff = (i == end - 1).then_some(end_fragment).flatten();
            if let Some(fragment) = fragment.take() {
                traverse_body_from_id(
                    body,
                    fragment,
                    &mut cb,
                    &styles,
                    &rules,
                    Style::empty(),
                    cutoff,
                )?;
            } else {
                traverse_body(body, &mut cb, &styles, &rules, Style::empty(), cutoff)?;
            }
        }

        Ok((self.title(), self.toc.0[entry].name.as_ref()))
    }

    pub fn title(&self) -> &str {
        &self.metadata.title
    }
}

fn update_style(
    styles: &StyleSheet,
    rules: &[(usize, Style)],
    node: Node,
    mut style: Style,
) -> Style {
    // TODO apply style from inline style attribute
    for added_style in rules.iter().filter_map(|&(i, style)| {
        styles.rules[i]
            .selector
            .matches(&XmlNode(node))
            .then_some(style)
    }) {
        style |= added_style;
    }
    match node.tag_name().name() {
        "i" | "em" => style |= Style::ITALIC,
        "b" | "strong" => style |= Style::BOLD,
        "center" => style |= Style::CENTER,
        _ => {}
    }
    style
}

fn traverse_body_from_id(
    node: Node,
    id: &str,
    cb: &mut impl FnMut(Content<'_>),
    styles: &StyleSheet,
    rules: &[(usize, Style)],
    style: Style,
    end: Option<&str>,
) -> anyhow::Result<Option<bool>> {
    if node.attribute("id") == Some(id) {
        if traverse_body(node, cb, styles, rules, Style::empty(), end)? {
            return Ok(None);
        };
        return Ok(Some(true));
    }
    let style = update_style(styles, rules, node, style);
    let mut found = false;
    for child in node.children() {
        match found {
            true => {
                if traverse_body(child, cb, styles, rules, Style::empty(), end)? {
                    return Ok(None);
                }
            }
            false => {
                found = match traverse_body_from_id(child, id, cb, styles, rules, style, end)? {
                    Some(f) => f,
                    None => {
                        return Ok(None);
                    }
                }
            }
        }
    }
    Ok(Some(found))
}

fn traverse_body(
    node: roxmltree::Node,
    cb: &mut impl FnMut(Content<'_>),
    styles: &StyleSheet,
    rules: &[(usize, Style)],
    style: Style,
    end: Option<&str>,
) -> anyhow::Result<bool> {
    fn recurse(
        node: roxmltree::Node,
        cb: &mut impl FnMut(Content<'_>),
        styles: &StyleSheet,
        rules: &[(usize, Style)],
        style: Style,
        end: Option<&str>,
    ) -> anyhow::Result<bool> {
        for node in node.children() {
            if traverse_body(node, cb, styles, rules, style, end)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    if node.is_text() {
        let text = node.text().context("invalid text node")?;
        if !text.trim().is_empty() {
            let content = Content::Text(style, text);
            cb(content);
        }
        return Ok(false);
    }

    if matches!((node.attribute("id"), end), (Some(a), Some(b)) if a == b) {
        // bail
        return Ok(true);
    }

    let style = update_style(styles, rules, node, style);

    match node.tag_name().name() {
        // "a" if node.has_attribute("href") => _ = recurse(node, cb, styles, rules, style, end)?,
        "p" | "div" | "blockquote" | "br" => {
            recurse(node, cb, styles, rules, style, end)?;
            cb(Content::Linebreak)
        }
        "img" if node.has_attribute("src") => cb(Content::Image),
        "image" => cb(Content::Image),
        _ => _ = recurse(node, cb, styles, rules, style, end)?,
    }
    Ok(false)
}

fn parse_hyperlink(base: &str, href: &str) -> anyhow::Result<Url> {
    Ok(Url::parse("epub:/")?.join(base)?.join(href)?)
}

// TODO save previews so can incremental search
// TODO iterator for multiple results
pub trait SearchBackend {
    fn search(&self, title: &str) -> anyhow::Result<Option<Epub>>;
}

pub struct Directory {
    dir: std::path::PathBuf,
}

impl SearchBackend for Directory {
    fn search(&self, title: &str) -> anyhow::Result<Option<Epub>> {
        let parse = |entry: fs::DirEntry| -> anyhow::Result<Option<Epub>> {
            match entry
                .path()
                .extension()
                .map(std::ffi::OsStr::to_string_lossy)
                .as_deref()
            {
                Some("epub") => {}
                _ => anyhow::bail!("not an epub: `{}`", entry.path().to_string_lossy()),
            }
            let doc = EpubPreview::from_file(entry.path())?;
            match doc.title().to_lowercase().contains(&title.to_lowercase()) {
                true => doc.full().map(Option::Some),
                false => Ok(None),
            }
        };
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;

            match parse(entry) {
                Ok(Some(doc)) => return Ok(Some(doc)),
                Ok(None) => {}
                Err(e) => eprintln!("failed to parse: {e}"),
            }
        }
        Ok(None)
    }
}

impl Directory {
    pub fn from_path(dir: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            dir,
        })
    }

    pub fn from_home() -> anyhow::Result<Self> {
        Ok(Self {
            dir: ebook_directory()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    first: String,
    middles: Option<String>,
    surname: String,
}

impl Author {
    fn parse(raw: &str) -> Option<Self> {
        let mut raw = raw.trim();
        if raw.is_empty() || raw == "Unknown" {
            return None;
        };
        let block_caps = !raw.chars().any(char::is_lowercase);
        // TODO: haven't tested this yet
        let mut buf = String::new();
        if !block_caps {
            let (mut first, mut second) = (raw.chars(), raw.chars().skip(1));
            while let (Some(a), Some(b)) = (first.next(), second.next()) {
                buf.push(a);
                if a.is_uppercase() && b.is_uppercase() {
                    buf.push(' ');
                }
            }
            buf.push(raw.chars().last().unwrap());
            raw = &buf;
        }
        let name = raw.to_lowercase();
        let name = name.replace(". ", " ");
        let name = name.replace('.', " ");
        let name = name.trim();
        let comma_count = name.matches(',').count();
        let reversed = comma_count % 2 == 1;
        let (given, surname) = match (comma_count, reversed) {
            (0, _) => name.rsplit_once(' ').unwrap_or((name, "")),
            (_, true) => name.split_once(',').map(|(a, b)| (b, a)).unwrap(),
            (_, false) => name.split_once(',').unwrap(),
        };
        let mut given = given.trim();
        let mut surname = surname.trim();
        let middles = if let Some((middles, real_surname)) = surname.rsplit_once(' ') {
            surname = real_surname.trim_start();
            Some(middles)
        } else if let Some((first, middles)) = given.split_once(' ') {
            given = first.trim_end();
            Some(middles)
        } else {
            None
        };
        fn capitalise(s: &str) -> String {
            let mut buf = String::new();
            for word in s.split_whitespace() {
                for ch in word.chars().next().unwrap().to_uppercase() {
                    buf.push(ch);
                }
                if word.len() == 1 {
                    buf.push_str(". ");
                } else {
                    for ch in word.chars().skip(1) {
                        buf.push(ch);
                    }
                }
            }
            while buf.chars().last().unwrap().is_whitespace() {
                buf.pop();
            }
            buf
        }
        Some(Self {
            first: capitalise(given.trim()),
            middles: middles.map(str::trim).map(capitalise),
            surname: capitalise(surname.trim_matches(',').trim()),
        })
    }
    // }
}

impl std::fmt::Display for Author {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.first)?;
        f.write_str(" ")?;
        if let Some(middles) = &self.middles {
            f.write_str(middles)?;
            f.write_str(" ")?;
        }
        f.write_str(&self.surname)
    }
}
