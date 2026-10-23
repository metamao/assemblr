//! The value-level merge that composes several amendments of one JSON or
//! JSONC file (see `docs/domain/amendments-and-composition.md` and
//! `docs/adr/0015-value-merge.md`).
//!
//! The engine sees a document as a [`Value`] tree and knows no text: an
//! adapter parses each side and writes the planned edits back, keeping the
//! file's layout. Composition runs in three steps, each pure:
//!
//! 1. [`derive`]: the operations that turn an amendment's base into its
//!    copy. Objects are compared member by member, scalars and lists as
//!    whole values, so a change records the value it expects to find.
//! 2. [`combine`]: the operations of every amender on one path. Equal
//!    operations are one; different ones are a conflict naming every
//!    amender, and no amender takes precedence. A removal or replacement
//!    cannot coexist with another amender's edit inside it.
//! 3. [`plan`]: the combined operations checked against the provider's
//!    file, which holds the base when every amender's provider resolves to
//!    it, and turned into edits on it, together with the value the edited
//!    file must hold.
//!
//! Lists are compared whole: identity of list items is never inferred, so
//! two amenders that change one list differently conflict, and changes to
//! the objects around a list are unaffected (OC-4 stays open). The one
//! exception is a list of scalars that every amender only appends to, or
//! creates: in a composition of units its items are joined, the provider's
//! first and then each amender's new ones in amender order, without
//! duplicates, since equal scalars are the same item ([`Lists::Union`]).
//! Object
//! member order carries no meaning in JSON, so members added at the same
//! place by several amenders are placed in a fixed order, by amender, which
//! never changes the document's value (OC-12 stays open for formats where
//! order means something). Every result is deterministic: amenders,
//! operations, and conflicts are ordered, never taken in the order they were
//! read.

use std::collections::BTreeMap;
use std::fmt;

use super::identity::UnitIdentity;
use super::path::RelativePath;

/// A JSON document as a value. Object members keep their order, so that
/// an amendment's placement of a new member can be followed, and never
/// repeat a key: the adapter rejects duplicate keys before a value is built.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Value {
    Null,
    Bool(bool),
    /// A number, as its canonical JSON text, so that `1.50` and `1.5` are
    /// one value and large integers lose nothing.
    Number(String),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// The member `key` of an object, or `None` for another kind of value
    /// or a missing member.
    pub fn member(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Object(members) => members
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    fn member_mut(&mut self, key: &str) -> Option<&mut Value> {
        match self {
            Self::Object(members) => members
                .iter_mut()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// The value at `path`, or `None` where the path leaves the document.
    pub fn at(&self, path: &[String]) -> Option<&Value> {
        path.iter().try_fold(self, |value, key| value.member(key))
    }

    fn at_mut(&mut self, path: &[String]) -> Option<&mut Value> {
        path.iter()
            .try_fold(self, |value, key| value.member_mut(key))
    }

    /// Whether the value is an object, which members can be added to.
    pub fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }

    /// A short rendering for diagnostics: the canonical JSON, cut after 40
    /// characters.
    pub fn brief(&self) -> String {
        let text = self.to_string();
        let mut chars = text.chars();
        let short: String = chars.by_ref().take(40).collect();
        if chars.next().is_some() {
            format!("{short}…")
        } else {
            short
        }
    }
}

impl fmt::Display for Value {
    /// Canonical JSON: no whitespace, members in order, strings escaped as
    /// JSON requires.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("null"),
            Self::Bool(value) => write!(f, "{value}"),
            Self::Number(text) => f.write_str(text),
            Self::String(text) => write_json_string(f, text),
            Self::Array(items) => {
                f.write_str("[")?;
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        f.write_str(",")?;
                    }
                    item.fmt(f)?;
                }
                f.write_str("]")
            }
            Self::Object(members) => {
                f.write_str("{")?;
                for (index, (key, value)) in members.iter().enumerate() {
                    if index > 0 {
                        f.write_str(",")?;
                    }
                    write_json_string(f, key)?;
                    f.write_str(":")?;
                    value.fmt(f)?;
                }
                f.write_str("}")
            }
        }
    }
}

/// Writes `text` as a JSON string literal.
pub fn write_json_string(f: &mut fmt::Formatter<'_>, text: &str) -> fmt::Result {
    f.write_str("\"")?;
    for c in text.chars() {
        match c {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\r' => f.write_str("\\r")?,
            '\t' => f.write_str("\\t")?,
            '\u{8}' => f.write_str("\\b")?,
            '\u{c}' => f.write_str("\\f")?,
            c if (c as u32) < 0x20 => write!(f, "\\u{:04x}", c as u32)?,
            c => write!(f, "{c}")?,
        }
    }
    f.write_str("\"")
}

/// The path of a value: the keys from the root, each an object member.
/// Lists are never entered, since their items have no identity here.
type Path = Vec<String>;

/// A path rendered for diagnostics, as `/a/b`, or `/` for the root.
pub fn show_path(path: &[String]) -> String {
    if path.is_empty() {
        return "/".to_owned();
    }
    path.iter().map(|key| format!("/{key}")).collect()
}

/// How an amendment changes the value at a path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValueChange {
    /// An object must exist here; its members come as their own
    /// operations, so that two amenders creating the same object share it.
    Ensure,
    /// A value is added where the base has none.
    Add(Value),
    /// The base's value is replaced.
    ValueChange {
        /// The value the base holds, which the file must still hold.
        from: Value,
        /// The new value.
        to: Value,
    },
    /// The base's value is removed.
    Remove(Value),
}

impl ValueChange {
    fn describe(&self) -> String {
        match self {
            Self::Ensure => "creates an object".to_owned(),
            Self::Add(value) => format!("adds {}", value.brief()),
            Self::ValueChange { from, to } => format!("changes {} to {}", from.brief(), to.brief()),
            Self::Remove(value) => format!("removes {}", value.brief()),
        }
    }
}

/// One operation of one amender: a change at a path, with the member the
/// copy places a new member after, when it adds one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    path: Path,
    change: ValueChange,
    /// For an addition, the key of the member the copy puts it after, or
    /// `None` when it is the first member.
    after: Option<String>,
}

impl Operation {
    /// The path of the changed value.
    pub fn path(&self) -> &[String] {
        &self.path
    }

    /// The change.
    pub fn change(&self) -> &ValueChange {
        &self.change
    }
}

/// The operations that turn `base` into `copy`, in document order, and
/// what the operations cannot carry: the member orders of `copy` that
/// differ from the base's.
pub fn derive(base: &Value, copy: &Value) -> Derived {
    let mut derived = Derived {
        operations: Vec::new(),
        reordered: Vec::new(),
    };
    diff(base, copy, &Vec::new(), &mut derived);
    derived
}

/// What [`derive`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derived {
    /// The operations.
    pub operations: Vec<Operation>,
    /// The objects whose common members the copy lists in another order,
    /// which no operation expresses.
    pub reordered: Vec<Path>,
}

fn diff(base: &Value, copy: &Value, path: &Path, out: &mut Derived) {
    match (base, copy) {
        (Value::Object(base_members), Value::Object(copy_members)) => {
            let base_keys: Vec<&String> = base_members.iter().map(|(key, _)| key).collect();
            for (index, (key, copy_value)) in copy_members.iter().enumerate() {
                let mut member_path = path.clone();
                member_path.push(key.clone());
                match base.member(key) {
                    None => {
                        // Placed after the nearest preceding member the
                        // base also has, so the anchor exists in the file.
                        let after = copy_members[..index]
                            .iter()
                            .rev()
                            .find(|(name, _)| base_keys.contains(&name))
                            .map(|(name, _)| name.clone());
                        add(&member_path, copy_value, after, out);
                    }
                    Some(base_value) if base_value != copy_value => {
                        diff(base_value, copy_value, &member_path, out);
                    }
                    Some(_) => {}
                }
            }
            for (key, base_value) in base_members {
                if copy.member(key).is_none() {
                    let mut member_path = path.clone();
                    member_path.push(key.clone());
                    out.operations.push(Operation {
                        path: member_path,
                        change: ValueChange::Remove(base_value.clone()),
                        after: None,
                    });
                }
            }
            let common_in_base: Vec<&String> = base_keys
                .iter()
                .copied()
                .filter(|key| copy.member(key).is_some())
                .collect();
            let common_in_copy: Vec<&String> = copy_members
                .iter()
                .map(|(key, _)| key)
                .filter(|key| base.member(key).is_some())
                .collect();
            if common_in_base != common_in_copy {
                out.reordered.push(path.clone());
            }
        }
        _ => {
            if base != copy {
                out.operations.push(Operation {
                    path: path.clone(),
                    change: ValueChange::ValueChange {
                        from: base.clone(),
                        to: copy.clone(),
                    },
                    after: None,
                });
            }
        }
    }
}

/// A new value: a non-empty object becomes an `Ensure` plus one addition
/// per member, so that two amenders creating the same object merge their
/// members instead of colliding on the object.
fn add(path: &Path, value: &Value, after: Option<String>, out: &mut Derived) {
    match value {
        Value::Object(members) if !members.is_empty() => {
            out.operations.push(Operation {
                path: path.clone(),
                change: ValueChange::Ensure,
                after,
            });
            let mut previous: Option<String> = None;
            for (key, member_value) in members {
                let mut member_path = path.clone();
                member_path.push(key.clone());
                add(&member_path, member_value, previous.clone(), out);
                previous = Some(key.clone());
            }
        }
        _ => out.operations.push(Operation {
            path: path.clone(),
            change: ValueChange::Add(value.clone()),
            after,
        }),
    }
}

/// An operation shared by the amenders that made it, in amender order.
/// `A` names a contributor: a unit that amends a file, or, in a consumer
/// project's reconciliation, one side of the three-way merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combined<A = UnitIdentity> {
    operation: Operation,
    amenders: Vec<A>,
}

impl<A> Combined<A> {
    /// The operation.
    pub fn operation(&self) -> &Operation {
        &self.operation
    }

    /// Who makes it, in contributor order.
    pub fn amenders(&self) -> &[A] {
        &self.amenders
    }
}

/// How [`combine`] treats one list that several amenders change
/// differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lists {
    /// The changes conflict: lists are compared whole.
    Whole,
    /// When every change only appends scalars to a list of scalars, or
    /// creates one, the items are joined: the base's first, then each
    /// amender's new ones in amender order, each item once. Any other
    /// change, such as a removal, a reorder, or an object item, is compared
    /// whole.
    Union,
}

/// Combines the operations of every amender. `sets` holds each amender's
/// operations; the amenders are taken in order of identity, whatever order
/// they are given in. Equal operations on one path are one; different ones
/// are a conflict, unless `lists` joins them, as is an edit inside a value
/// another amender removes or replaces with a scalar or a list.
pub fn combine<A: Ord + Clone>(
    sets: &BTreeMap<A, Vec<Operation>>,
    lists: Lists,
) -> Result<Vec<Combined<A>>, Vec<Conflict<A>>> {
    let mut by_path: BTreeMap<Path, Vec<(A, &Operation)>> = BTreeMap::new();
    for (amender, operations) in sets {
        for operation in operations {
            by_path
                .entry(operation.path.clone())
                .or_default()
                .push((amender.clone(), operation));
        }
    }
    let mut combined = Vec::new();
    let mut conflicts = Vec::new();
    for (path, operations) in &by_path {
        let mut distinct: Vec<Combined<A>> = Vec::new();
        for (amender, operation) in operations {
            // Two `Ensure`s differ at most in their anchor, which is not a
            // conflict: the object is shared, its members are placed by
            // their own operations.
            let same = distinct.iter_mut().find(|existing| {
                existing.operation.change == operation.change
                    && (operation.change == ValueChange::Ensure
                        || existing.operation.after == operation.after)
            });
            match same {
                Some(existing) => existing.amenders.push(amender.clone()),
                None => distinct.push(Combined {
                    operation: (*operation).clone(),
                    amenders: vec![amender.clone()],
                }),
            }
        }
        if distinct.len() == 1 {
            combined.extend(distinct);
            continue;
        }
        if lists == Lists::Union {
            if let Some(joined) = join_appends(&distinct) {
                combined.push(joined);
                continue;
            }
        }
        // The same addition placed differently by two amenders is still
        // the same addition: it is placed as the first amender places it.
        let additions_agree = distinct.iter().all(|entry| {
            matches!(entry.operation.change, ValueChange::Add(_))
                && entry.operation.change == distinct[0].operation.change
        });
        if additions_agree {
            let mut merged = distinct.remove(0);
            for other in distinct {
                merged.amenders.extend(other.amenders);
            }
            merged.amenders.sort();
            merged.amenders.dedup();
            combined.push(merged);
            continue;
        }
        conflicts.push(Conflict {
            path: path.clone(),
            kind: ConflictKind::Different {
                changes: distinct
                    .into_iter()
                    .map(|entry| (entry.amenders, entry.operation.change))
                    .collect(),
            },
        });
    }
    // A removed value, or one replaced by something that is not an object,
    // cannot also receive another amender's edits inside it.
    let replacing: Vec<&Combined<A>> = combined
        .iter()
        .filter(|entry| match &entry.operation.change {
            ValueChange::Remove(_) => true,
            ValueChange::ValueChange { to, .. } => !to.is_object(),
            ValueChange::Ensure | ValueChange::Add(_) => false,
        })
        .collect();
    for outer in replacing {
        for inner in &combined {
            if inner.operation.path.len() > outer.operation.path.len()
                && inner.operation.path.starts_with(&outer.operation.path)
                && inner.amenders != outer.amenders
            {
                conflicts.push(Conflict {
                    path: outer.operation.path.clone(),
                    kind: ConflictKind::EditedInside {
                        amenders: outer.amenders.clone(),
                        change: outer.operation.change.clone(),
                        inside: inner.operation.path.clone(),
                        by: inner.amenders.clone(),
                    },
                });
            }
        }
    }
    if conflicts.is_empty() {
        Ok(combined)
    } else {
        conflicts.sort();
        Err(conflicts)
    }
}

/// The appends of `distinct`, the different changes amenders make to one
/// path in amender order, joined into one change, or `None` when one of
/// them is not an append of new, distinct scalars to the same list of
/// scalars, or the creation of such a list.
fn join_appends<A: Ord + Clone>(distinct: &[Combined<A>]) -> Option<Combined<A>> {
    let is_scalar = |value: &Value| !matches!(value, Value::Array(_) | Value::Object(_));
    let (from, base) = match &distinct.first()?.operation.change {
        ValueChange::ValueChange {
            from: from @ Value::Array(base),
            ..
        } => (Some(from), base.as_slice()),
        ValueChange::Add(_) => (None, [].as_slice()),
        _ => return None,
    };
    if !base.iter().all(is_scalar) {
        return None;
    }
    let mut items = base.to_vec();
    let mut amenders = Vec::new();
    for entry in distinct {
        let copy = match (&entry.operation.change, from) {
            (ValueChange::ValueChange { from: own, to }, Some(from)) if own == from => to,
            (ValueChange::Add(value), None) => value,
            _ => return None,
        };
        let Value::Array(copy) = copy else {
            return None;
        };
        let appended = copy.strip_prefix(base)?;
        for (index, item) in appended.iter().enumerate() {
            if !is_scalar(item) || base.contains(item) || appended[..index].contains(item) {
                return None;
            }
            if !items.contains(item) {
                items.push(item.clone());
            }
        }
        amenders.extend(entry.amenders.iter().cloned());
    }
    amenders.sort();
    amenders.dedup();
    let joined = Value::Array(items);
    let change = match from {
        Some(from) => ValueChange::ValueChange {
            from: from.clone(),
            to: joined,
        },
        None => ValueChange::Add(joined),
    };
    Some(Combined {
        operation: Operation {
            path: distinct[0].operation.path.clone(),
            change,
            after: distinct[0].operation.after.clone(),
        },
        amenders,
    })
}

/// An edit to make on the provider's file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Adds the member `key` with `value` to the object at `parent`, after
    /// the member `after`, or first when `after` is `None`.
    Insert {
        parent: Path,
        key: String,
        value: Value,
        after: Option<String>,
    },
    /// Replaces the value at `path`.
    Set { path: Path, value: Value },
    /// Removes the member at `path`.
    Delete { path: Path },
}

/// What [`plan`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The edits, in the order they apply: parents before their members,
    /// and members added after one anchor in amender order, each after the
    /// one before it.
    pub edits: Vec<Edit>,
    /// The value the edited file must hold.
    pub result: Value,
}

/// Checks the combined operations against `target`, the provider's file,
/// and turns them into edits. A value that is not what an operation
/// expects is a conflict: the file changed under the amendment.
pub fn plan<A: Ord + Clone>(
    target: &Value,
    mut operations: Vec<Combined<A>>,
) -> Result<Plan, Vec<Conflict<A>>> {
    // Parents before members; within one parent, by anchor, then by the
    // first amender, then by key, so that the order never depends on the
    // order the operations were derived in.
    operations.sort_by(|a, b| {
        let key = |entry: &Combined<A>| {
            (
                entry.operation.path.len(),
                entry.operation.path[..entry.operation.path.len().saturating_sub(1)].to_vec(),
                entry.operation.after.clone(),
                entry.amenders.clone(),
                entry.operation.path.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
    let mut result = target.clone();
    let mut edits = Vec::new();
    let mut conflicts = Vec::new();
    // The last member inserted after each anchor, so that the next one
    // inserted after the same anchor follows it.
    let mut chain: BTreeMap<(Path, Option<String>), String> = BTreeMap::new();
    for entry in operations {
        let path = &entry.operation.path;
        let Some((key, parent)) = path.split_last() else {
            // The root itself: only a whole-document change.
            match &entry.operation.change {
                ValueChange::ValueChange { from, to } if *from == result => {
                    edits.push(Edit::Set {
                        path: Vec::new(),
                        value: to.clone(),
                    });
                    result = to.clone();
                }
                ValueChange::ValueChange { to, .. } if *to == result => {}
                change => conflicts.push(Conflict {
                    path: Vec::new(),
                    kind: ConflictKind::Unexpected {
                        amenders: entry.amenders.clone(),
                        change: change.clone(),
                        found: Some(result.clone()),
                    },
                }),
            }
            continue;
        };
        let found = result.at(path).cloned();
        let parent_is_object = result.at(parent).is_some_and(Value::is_object);
        let unexpected = |found: Option<Value>| Conflict {
            path: path.clone(),
            kind: ConflictKind::Unexpected {
                amenders: entry.amenders.clone(),
                change: entry.operation.change.clone(),
                found,
            },
        };
        let mut insert = |value: Value, result: &mut Value, edits: &mut Vec<Edit>| {
            let anchor = (parent.to_vec(), entry.operation.after.clone());
            let after = chain
                .get(&anchor)
                .cloned()
                .or_else(|| entry.operation.after.clone());
            chain.insert(anchor, key.clone());
            edits.push(Edit::Insert {
                parent: parent.to_vec(),
                key: key.clone(),
                value: value.clone(),
                after: after.clone(),
            });
            if let Some(Value::Object(members)) = result.at_mut(parent) {
                let position = after
                    .and_then(|after| members.iter().position(|(name, _)| *name == after))
                    .map_or(0, |index| index + 1);
                members.insert(position, (key.clone(), value));
            }
        };
        match &entry.operation.change {
            ValueChange::Ensure => match found {
                None if parent_is_object => {
                    insert(Value::Object(Vec::new()), &mut result, &mut edits)
                }
                Some(Value::Object(_)) => {}
                found => conflicts.push(unexpected(found)),
            },
            ValueChange::Add(value) => match found {
                None if parent_is_object => insert(value.clone(), &mut result, &mut edits),
                Some(existing) if existing == *value => {}
                found => conflicts.push(unexpected(found)),
            },
            ValueChange::ValueChange { from, to } => match found {
                Some(existing) if existing == *from => {
                    edits.push(Edit::Set {
                        path: path.clone(),
                        value: to.clone(),
                    });
                    if let Some(slot) = result.at_mut(path) {
                        *slot = to.clone();
                    }
                }
                Some(existing) if existing == *to => {}
                found => conflicts.push(unexpected(found)),
            },
            ValueChange::Remove(from) => match found {
                Some(existing) if existing == *from => {
                    edits.push(Edit::Delete { path: path.clone() });
                    if let Some(Value::Object(members)) = result.at_mut(parent) {
                        members.retain(|(name, _)| name != key);
                    }
                }
                None => {}
                found => conflicts.push(unexpected(found)),
            },
        }
    }
    if conflicts.is_empty() {
        Ok(Plan { edits, result })
    } else {
        conflicts.sort();
        Err(conflicts)
    }
}

/// Why amendments cannot be combined, at one path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Conflict<A = UnitIdentity> {
    path: Path,
    kind: ConflictKind<A>,
}

impl<A> Conflict<A> {
    /// The path of the contested value.
    pub fn path(&self) -> &[String] {
        &self.path
    }

    /// What is in conflict.
    pub fn kind(&self) -> &ConflictKind<A> {
        &self.kind
    }
}

/// The kinds of conflict.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConflictKind<A = UnitIdentity> {
    /// Amenders make different changes to one value. Each change with the
    /// amenders that make it, in amender order.
    Different { changes: Vec<(Vec<A>, ValueChange)> },
    /// Amenders remove or replace a value that other amenders edit inside.
    EditedInside {
        amenders: Vec<A>,
        change: ValueChange,
        inside: Path,
        by: Vec<A>,
    },
    /// The provider's file does not hold what the change expects.
    Unexpected {
        amenders: Vec<A>,
        change: ValueChange,
        /// What the file holds there, or `None` for nothing.
        found: Option<Value>,
    },
}

impl<A: fmt::Display> fmt::Display for Conflict<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = |amenders: &[A]| {
            amenders
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" and ")
        };
        write!(f, "{}: ", show_path(&self.path))?;
        match &self.kind {
            ConflictKind::Different { changes } => {
                for (index, (amenders, change)) in changes.iter().enumerate() {
                    if index > 0 {
                        f.write_str("; ")?;
                    }
                    write!(f, "{} {}", names(amenders), change.describe())?;
                }
                Ok(())
            }
            ConflictKind::EditedInside {
                amenders,
                change,
                inside,
                by,
            } => write!(
                f,
                "{} {}, while {} edits {} inside it",
                names(amenders),
                change.describe(),
                names(by),
                show_path(inside)
            ),
            ConflictKind::Unexpected {
                amenders,
                change,
                found,
            } => {
                write!(
                    f,
                    "{} {}, but the provider's file ",
                    names(amenders),
                    change.describe()
                )?;
                match found {
                    Some(found) => write!(f, "holds {} there", found.brief()),
                    None => f.write_str("has nothing there"),
                }
            }
        }
    }
}

impl<A: fmt::Debug + fmt::Display> std::error::Error for Conflict<A> {}

/// How several amendments of one file are combined, by the file's format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeFormat {
    /// JSON, or JSONC with comments and trailing commas: combined by value.
    Json,
    /// Text: combined by line, when the files are UTF-8 text. YAML, TOML,
    /// and INI are merged as text too (owner decision on #92), accepting
    /// that a clean line merge can leave a key twice.
    Text,
    /// A structured format with no merge (XML, and JSON spelled with another
    /// case), so amendments of one such file by several units stay refused.
    Unsupported,
}

/// The extensions of structured formats with no merge.
const UNSUPPORTED_EXTENSIONS: [&str; 1] = ["xml"];

/// The extensions of structured formats that unit composition merges by
/// line but a consumer's reconciliation still treats as having no merge.
const LINE_MERGED_EXTENSIONS: [&str; 4] = ["yaml", "yml", "toml", "ini"];

impl MergeFormat {
    /// The format of the file delivered at `path`, by its extension, for
    /// combining the amendments of several units or applying one across
    /// provider versions.
    pub fn of_path(path: &RelativePath) -> Self {
        match extension(path) {
            Some("json" | "jsonc") => Self::Json,
            Some(extension)
                if extension.eq_ignore_ascii_case("json")
                    || extension.eq_ignore_ascii_case("jsonc")
                    || is_one_of(extension, &UNSUPPORTED_EXTENSIONS) =>
            {
                Self::Unsupported
            }
            _ => Self::Text,
        }
    }

    /// The format of the project file at `path` for a consumer's
    /// reconciliation: as [`MergeFormat::of_path`], except that YAML, TOML,
    /// and INI have no merge there, so a file both the consumer and the
    /// composition changed is marked whole.
    pub fn of_project_path(path: &RelativePath) -> Self {
        match extension(path) {
            Some(extension) if is_one_of(extension, &LINE_MERGED_EXTENSIONS) => Self::Unsupported,
            _ => Self::of_path(path),
        }
    }
}

fn extension(path: &RelativePath) -> Option<&str> {
    let name = path.segments().last().unwrap_or("");
    name.rsplit_once('.').map(|(_, extension)| extension)
}

fn is_one_of(extension: &str, known: &[&str]) -> bool {
    known
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "fixtures are valid by construction")]

    use std::collections::BTreeMap;

    use super::{
        Combined, Conflict, ConflictKind, Edit, Lists, MergeFormat, Operation, Path, Value,
        ValueChange, combine, derive, plan,
    };
    use crate::domain::{RelativePath, UnitIdentity};

    fn identity(input: &str) -> UnitIdentity {
        input.parse().unwrap()
    }

    fn s(text: &str) -> Value {
        Value::String(text.to_owned())
    }

    fn n(text: &str) -> Value {
        Value::Number(text.to_owned())
    }

    fn object(members: &[(&str, Value)]) -> Value {
        Value::Object(
            members
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        )
    }

    fn path(text: &str) -> Path {
        if text == "/" {
            Vec::new()
        } else {
            text.trim_start_matches('/')
                .split('/')
                .map(str::to_owned)
                .collect()
        }
    }

    /// A `package.json`-like base: scripts and dependencies.
    fn package() -> Value {
        object(&[
            ("name", s("app")),
            ("private", Value::Bool(true)),
            (
                "scripts",
                object(&[("build", s("tsc")), ("dev", s("wrangler dev"))]),
            ),
            ("dependencies", object(&[("hono", s("^4"))])),
            ("files", Value::Array(vec![s("dist")])),
        ])
    }

    fn with<F: FnOnce(&mut Vec<(String, Value)>)>(base: &Value, edit: F) -> Value {
        let Value::Object(mut members) = base.clone() else {
            unreachable!("the fixtures are objects");
        };
        edit(&mut members);
        Value::Object(members)
    }

    fn members_mut<'a>(
        members: &'a mut [(String, Value)],
        key: &str,
    ) -> &'a mut Vec<(String, Value)> {
        match &mut members.iter_mut().find(|(k, _)| k == key).unwrap().1 {
            Value::Object(inner) => inner,
            _ => unreachable!("an object member"),
        }
    }

    fn merge(base: &Value, copies: &[(&str, Value)]) -> Result<(Vec<Edit>, Value), Vec<Conflict>> {
        let sets: BTreeMap<UnitIdentity, Vec<Operation>> = copies
            .iter()
            .map(|(amender, copy)| (identity(amender), derive(base, copy).operations))
            .collect();
        let combined = combine(&sets, Lists::Union)?;
        plan(base, combined).map(|plan| (plan.edits, plan.result))
    }

    #[test]
    fn derive_finds_additions_changes_and_removals_with_their_anchors() {
        let base = package();
        let copy = with(&base, |members| {
            let scripts = members_mut(members, "scripts");
            scripts.insert(1, ("check".to_owned(), s("tsc --noEmit")));
            scripts[0].1 = s("tsc -p .");
            members.retain(|(key, _)| key != "private");
            members.push((
                "peerDependencies".to_owned(),
                object(&[("react", s("^19"))]),
            ));
        });

        let derived = derive(&base, &copy);

        assert_eq!(
            derived.operations,
            [
                Operation {
                    path: path("/scripts/build"),
                    change: ValueChange::ValueChange {
                        from: s("tsc"),
                        to: s("tsc -p .")
                    },
                    after: None,
                },
                Operation {
                    path: path("/scripts/check"),
                    change: ValueChange::Add(s("tsc --noEmit")),
                    after: Some("build".to_owned()),
                },
                Operation {
                    path: path("/peerDependencies"),
                    change: ValueChange::Ensure,
                    after: Some("files".to_owned()),
                },
                Operation {
                    path: path("/peerDependencies/react"),
                    change: ValueChange::Add(s("^19")),
                    after: None,
                },
                Operation {
                    path: path("/private"),
                    change: ValueChange::Remove(Value::Bool(true)),
                    after: None,
                },
            ]
        );
        assert!(derived.reordered.is_empty());
    }

    #[test]
    fn independent_changes_of_several_amenders_are_all_applied() {
        let base = package();
        let api = with(&base, |members| {
            members_mut(members, "scripts").push(("test".to_owned(), s("vitest")));
            members_mut(members, "dependencies").push(("zod".to_owned(), s("^3")));
        });
        let db = with(&base, |members| {
            members_mut(members, "dependencies").insert(0, ("drizzle-orm".to_owned(), s("^0.44")));
            members.push((
                "devDependencies".to_owned(),
                object(&[("drizzle-kit", s("^0.31"))]),
            ));
        });

        let (edits, result) = merge(&base, &[("@acme/db", db), ("@acme/api", api)]).unwrap();

        assert_eq!(
            result,
            object(&[
                ("name", s("app")),
                ("private", Value::Bool(true)),
                (
                    "scripts",
                    object(&[
                        ("build", s("tsc")),
                        ("dev", s("wrangler dev")),
                        ("test", s("vitest")),
                    ]),
                ),
                (
                    "dependencies",
                    object(&[
                        ("drizzle-orm", s("^0.44")),
                        ("hono", s("^4")),
                        ("zod", s("^3"))
                    ]),
                ),
                ("files", Value::Array(vec![s("dist")])),
                ("devDependencies", object(&[("drizzle-kit", s("^0.31"))])),
            ])
        );
        assert_eq!(edits.len(), 5);
    }

    #[test]
    fn identical_contributions_are_one_change_and_different_ones_conflict() {
        let base = package();
        let bump = |to: &str| {
            with(&base, |members| {
                members_mut(members, "dependencies")[0].1 = s(to);
            })
        };

        let (edits, result) = merge(
            &base,
            &[("@acme/a", bump("^4.1")), ("@acme/b", bump("^4.1"))],
        )
        .unwrap();
        assert_eq!(
            edits,
            [Edit::Set {
                path: path("/dependencies/hono"),
                value: s("^4.1")
            }]
        );
        assert_eq!(result.at(&path("/dependencies/hono")), Some(&s("^4.1")));

        let conflicts = merge(
            &base,
            &[("@acme/b", bump("^4.2")), ("@acme/a", bump("^4.1"))],
        )
        .unwrap_err();
        assert_eq!(
            conflicts,
            [Conflict {
                path: path("/dependencies/hono"),
                kind: ConflictKind::Different {
                    changes: vec![
                        (
                            vec![identity("@acme/a")],
                            ValueChange::ValueChange {
                                from: s("^4"),
                                to: s("^4.1")
                            }
                        ),
                        (
                            vec![identity("@acme/b")],
                            ValueChange::ValueChange {
                                from: s("^4"),
                                to: s("^4.2")
                            }
                        ),
                    ]
                },
            }]
        );
        assert_eq!(
            conflicts[0].to_string(),
            "/dependencies/hono: @acme/a changes \"^4\" to \"^4.1\"; @acme/b changes \"^4\" to \"^4.2\""
        );
    }

    #[test]
    fn two_amenders_creating_the_same_object_share_it_and_a_value_added_by_one_cannot_be_changed_by_another()
     {
        let base = package();
        let a = with(&base, |members| {
            members.push(("engines".to_owned(), object(&[("node", s(">=22"))])));
        });
        let b = with(&base, |members| {
            members.insert(1, ("engines".to_owned(), object(&[("npm", s(">=10"))])));
        });
        let (_, result) = merge(&base, &[("@acme/a", a.clone()), ("@acme/b", b)]).unwrap();
        assert_eq!(
            result.at(&path("/engines")),
            Some(&object(&[("node", s(">=22")), ("npm", s(">=10"))]))
        );

        // The flat model: `node` is not in b's base, so setting it is a
        // conflict with a's addition, whatever b depends on.
        let b = with(&base, |members| {
            members.push(("engines".to_owned(), object(&[("node", s(">=24"))])));
        });
        let conflicts = merge(&base, &[("@acme/a", a), ("@acme/b", b)]).unwrap_err();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(
            conflicts[0].to_string(),
            "/engines/node: @acme/a adds \">=22\"; @acme/b adds \">=24\""
        );
    }

    #[test]
    fn a_removal_conflicts_with_a_change_and_with_an_edit_inside() {
        let base = package();
        let remove = with(&base, |members| members.retain(|(key, _)| key != "scripts"));
        let change = with(&base, |members| {
            members_mut(members, "scripts")[1].1 = s("wrangler dev --port 8787");
        });
        let conflicts =
            merge(&base, &[("@acme/a", remove.clone()), ("@acme/b", change)]).unwrap_err();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(
            conflicts[0].to_string(),
            "/scripts: @acme/a removes {\"build\":\"tsc\",\"dev\":\"wrangler dev\"}, while @acme/b \
             edits /scripts/dev inside it"
        );

        // Two amenders removing the same value agree.
        let (edits, result) =
            merge(&base, &[("@acme/a", remove.clone()), ("@acme/b", remove)]).unwrap();
        assert_eq!(
            edits,
            [Edit::Delete {
                path: path("/scripts")
            }]
        );
        assert!(result.member("scripts").is_none());
    }

    #[test]
    fn lists_are_compared_whole() {
        let base = package();
        let files = |items: &[&str]| {
            with(&base, |members| {
                members
                    .iter_mut()
                    .find(|(key, _)| key == "files")
                    .unwrap()
                    .1 = Value::Array(items.iter().map(|item| s(item)).collect());
            })
        };
        // Identical whole-list changes merge, and an object change beside
        // the list is unaffected.
        let other = with(&base, |members| {
            members_mut(members, "scripts").push(("lint".to_owned(), s("eslint")));
        });
        let (_, result) = merge(
            &base,
            &[
                ("@acme/a", files(&["dist", "README.md"])),
                ("@acme/b", files(&["dist", "README.md"])),
                ("@acme/c", other),
            ],
        )
        .unwrap();
        assert_eq!(
            result.at(&path("/files")),
            Some(&Value::Array(vec![s("dist"), s("README.md")]))
        );
        assert_eq!(result.at(&path("/scripts/lint")), Some(&s("eslint")));

        // Different changes that are not appends are not merged inside
        // the list: they conflict.
        let conflicts = merge(
            &base,
            &[
                ("@acme/a", files(&["a", "dist"])),
                ("@acme/b", files(&["dist", "b"])),
            ],
        )
        .unwrap_err();
        assert_eq!(conflicts.len(), 1);
        assert!(
            conflicts[0]
                .to_string()
                .starts_with("/files: @acme/a changes [\"dist\"] to [\"a\",\"dist\"]")
        );
    }

    #[test]
    fn appends_to_a_list_of_scalars_are_joined_in_amender_order() {
        let base = package();
        let files = |items: &[&str]| {
            with(&base, |members| {
                members
                    .iter_mut()
                    .find(|(key, _)| key == "files")
                    .unwrap()
                    .1 = Value::Array(items.iter().map(|item| s(item)).collect());
            })
        };
        let copies = [
            ("@acme/c", files(&["dist", "c", "shared"])),
            ("@acme/a", files(&["dist", "a"])),
            ("@acme/b", files(&["dist", "shared", "b"])),
        ];
        let joined = Value::Array(vec![s("dist"), s("a"), s("shared"), s("b"), s("c")]);
        // The provider's items first, then each amender's new ones in
        // identity order, each item once, whatever order the copies come in.
        let mut reversed = copies.clone();
        reversed.reverse();
        for copies in [&copies[..], &reversed[..]] {
            let (edits, result) = merge(&base, copies).unwrap();
            assert_eq!(
                edits,
                [Edit::Set {
                    path: path("/files"),
                    value: joined.clone()
                }]
            );
            assert_eq!(result.at(&path("/files")), Some(&joined));
        }

        // The joined change names every amender in a later conflict.
        let target = with(&base, |members| {
            members.retain(|(key, _)| key != "files");
        });
        let sets: BTreeMap<UnitIdentity, Vec<Operation>> = copies
            .iter()
            .map(|(amender, copy)| (identity(amender), derive(&base, copy).operations))
            .collect();
        let conflicts = plan(&target, combine(&sets, Lists::Union).unwrap()).unwrap_err();
        assert_eq!(conflicts.len(), 1);
        assert!(
            conflicts[0]
                .to_string()
                .starts_with("/files: @acme/a and @acme/b and @acme/c changes [\"dist\"] to")
        );

        // Compared whole, the same changes conflict.
        let conflicts = combine(&sets, Lists::Whole).unwrap_err();
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn lists_of_scalars_several_amenders_create_are_joined() {
        let base = object(&[("name", s("app"))]);
        let extensions = |items: &[&str]| {
            with(&base, |members| {
                members.push((
                    "customizations".to_owned(),
                    object(&[(
                        "vscode",
                        object(&[(
                            "extensions",
                            Value::Array(items.iter().map(|item| s(item)).collect()),
                        )]),
                    )]),
                ));
            })
        };
        let (_, result) = merge(
            &base,
            &[
                ("@acme/b", extensions(&["rust-lang.rust-analyzer"])),
                ("@acme/a", extensions(&["esbenp.prettier-vscode"])),
            ],
        )
        .unwrap();
        assert_eq!(
            result.at(&path("/customizations/vscode/extensions")),
            Some(&Value::Array(vec![
                s("esbenp.prettier-vscode"),
                s("rust-lang.rust-analyzer")
            ]))
        );
    }

    #[test]
    fn list_changes_that_are_not_appends_of_new_scalars_still_conflict() {
        let base = object(&[
            ("tags", Value::Array(vec![s("x"), s("y")])),
            ("items", Value::Array(vec![object(&[("id", n("1"))])])),
        ]);
        let set = |key: &str, items: Vec<Value>| {
            with(&base, |members| {
                members.iter_mut().find(|(name, _)| name == key).unwrap().1 = Value::Array(items);
            })
        };
        let append = set("tags", vec![s("x"), s("y"), s("z")]);
        let cases = [
            // A removal.
            set("tags", vec![s("x")]),
            // A reorder.
            set("tags", vec![s("y"), s("x"), s("w")]),
            // An insertion before the provider's last item.
            set("tags", vec![s("x"), s("w"), s("y")]),
            // An item the provider already has, or one appended twice.
            set("tags", vec![s("x"), s("y"), s("x")]),
            set("tags", vec![s("x"), s("y"), s("w"), s("w")]),
            // An object or a list appended.
            set("tags", vec![s("x"), s("y"), object(&[("k", n("1"))])]),
            set("tags", vec![s("x"), s("y"), Value::Array(vec![s("w")])]),
            // A replacement by something that is not a list.
            with(&base, |members| {
                members
                    .iter_mut()
                    .find(|(name, _)| name == "tags")
                    .unwrap()
                    .1 = s("x");
            }),
        ];
        for other in cases {
            let conflicts =
                merge(&base, &[("@acme/a", append.clone()), ("@acme/b", other)]).unwrap_err();
            assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        }

        // A list of objects is compared whole, even when both append.
        let first = set(
            "items",
            vec![object(&[("id", n("1"))]), object(&[("id", n("2"))])],
        );
        let second = set(
            "items",
            vec![object(&[("id", n("1"))]), object(&[("id", n("3"))])],
        );
        let conflicts = merge(&base, &[("@acme/a", first), ("@acme/b", second)]).unwrap_err();
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn a_change_the_provider_s_file_does_not_expect_is_a_conflict() {
        let base = package();
        let copy = with(&base, |members| {
            members_mut(members, "scripts")[0].1 = s("tsc -b");
        });
        // The file the plan is checked against differs from the base the
        // copy was derived from.
        let target = with(&base, |members| {
            members_mut(members, "scripts")[0].1 = s("rollup");
        });
        let sets = BTreeMap::from([(identity("@acme/a"), derive(&base, &copy).operations)]);
        let conflicts = plan(&target, combine(&sets, Lists::Union).unwrap()).unwrap_err();
        assert_eq!(
            conflicts[0].to_string(),
            "/scripts/build: @acme/a changes \"tsc\" to \"tsc -b\", but the provider's file holds \
             \"rollup\" there"
        );
    }

    #[test]
    fn members_added_after_one_anchor_are_placed_in_amender_order_whatever_the_input_order() {
        let base = package();
        let add = |key: &str| {
            with(&base, |members| {
                members_mut(members, "scripts").insert(1, (key.to_owned(), s(key)));
            })
        };
        let forward = merge(
            &base,
            &[
                ("@acme/a", add("a")),
                ("@acme/b", add("b")),
                ("@acme/c", add("c")),
            ],
        )
        .unwrap();
        let backward = merge(
            &base,
            &[
                ("@acme/c", add("c")),
                ("@acme/b", add("b")),
                ("@acme/a", add("a")),
            ],
        )
        .unwrap();
        assert_eq!(forward, backward);
        let scripts = forward.1.at(&path("/scripts")).unwrap();
        assert_eq!(
            scripts,
            &object(&[
                ("build", s("tsc")),
                ("a", s("a")),
                ("b", s("b")),
                ("c", s("c")),
                ("dev", s("wrangler dev")),
            ])
        );
        // Each is inserted after the one before it, so the editor needs no
        // position arithmetic.
        let inserts: Vec<(String, Option<String>)> = forward
            .0
            .iter()
            .filter_map(|edit| match edit {
                Edit::Insert { key, after, .. } => Some((key.clone(), after.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            inserts,
            [
                ("a".to_owned(), Some("build".to_owned())),
                ("b".to_owned(), Some("a".to_owned())),
                ("c".to_owned(), Some("b".to_owned())),
            ]
        );
    }

    #[test]
    fn reordered_members_are_reported_not_operations() {
        let base = package();
        let reordered = with(&base, |members| members.swap(0, 1));
        let derived = derive(&base, &reordered);
        assert!(derived.operations.is_empty());
        assert_eq!(derived.reordered, [Vec::<String>::new()]);
    }

    #[test]
    fn numbers_compare_by_canonical_text_and_values_display_as_canonical_json() {
        let value = object(&[
            ("a", n("1.5")),
            ("b", Value::Null),
            ("c", s("q\"\\\n")),
            ("d", Value::Array(vec![Value::Bool(false), n("-2")])),
        ]);
        assert_eq!(
            value.to_string(),
            r#"{"a":1.5,"b":null,"c":"q\"\\\n","d":[false,-2]}"#
        );
        assert_eq!(n("1.5"), n("1.5"));
        assert_ne!(n("1.5"), n("1.50"));
    }

    #[test]
    fn json_paths_merge_by_value_text_by_line_and_xml_not_at_all() {
        let format = |text: &str| MergeFormat::of_path(&RelativePath::parse(text).unwrap());
        assert_eq!(format("package.json"), MergeFormat::Json);
        assert_eq!(format("config/wrangler.jsonc"), MergeFormat::Json);
        assert_eq!(format("AGENTS.md"), MergeFormat::Text);
        assert_eq!(format("Dockerfile"), MergeFormat::Text);
        assert_eq!(format("src/server/api.ts"), MergeFormat::Text);
        assert_eq!(format(".gitignore"), MergeFormat::Text);
        // YAML, TOML, and INI are merged by line, in any case.
        for text in ["compose.yaml", "ci.YML", "Cargo.toml", "x.ini"] {
            assert_eq!(format(text), MergeFormat::Text, "{text}");
        }
        // Structured formats with no merge, in any case.
        for refused in ["pom.xml", "pom.XML", "notes.JSON"] {
            assert_eq!(format(refused), MergeFormat::Unsupported, "{refused}");
        }
    }

    #[test]
    fn a_consumer_reconciliation_still_has_no_merge_for_yaml_toml_and_ini() {
        let format = |text: &str| MergeFormat::of_project_path(&RelativePath::parse(text).unwrap());
        for refused in ["compose.yaml", "ci.YML", "Cargo.toml", "x.ini", "pom.xml"] {
            assert_eq!(format(refused), MergeFormat::Unsupported, "{refused}");
        }
        assert_eq!(format("package.json"), MergeFormat::Json);
        assert_eq!(format("AGENTS.md"), MergeFormat::Text);
    }

    #[test]
    fn a_combined_operation_keeps_its_amenders_in_identity_order() {
        let base = package();
        let copy = with(&base, |members| members.retain(|(key, _)| key != "private"));
        let sets = BTreeMap::from([
            (identity("@acme/z"), derive(&base, &copy).operations),
            (identity("@acme/a"), derive(&base, &copy).operations),
        ]);
        let combined = combine(&sets, Lists::Union).unwrap();
        assert_eq!(
            combined,
            [Combined {
                operation: Operation {
                    path: path("/private"),
                    change: ValueChange::Remove(Value::Bool(true)),
                    after: None,
                },
                amenders: vec![identity("@acme/a"), identity("@acme/z")],
            }]
        );
    }
}
