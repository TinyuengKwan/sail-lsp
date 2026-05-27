//! Maps syntax elements through disjoint syntax nodes.
//! [`SyntaxMappingBuilder`] should be used to create mappings to add to a `SyntaxEditor`.

use rustc_hash::FxHashMap;

use crate::syntax_node::{SyntaxElement, SyntaxNode};

/// Tracks mapping between nodes in an input tree and nodes in an output tree.
///
/// each mapped input node stores which parent it belongs to and its child slot index.
#[derive(Debug, Default, Clone)]
pub struct SyntaxMapping {
    /// Parent nodes that contain mapped children.
    entry_parents: Vec<SyntaxNode>,
    /// Maps input nodes to their location in the output tree.
    /// Uses SyntaxNode directly as key (rowan provides Hash + Eq).
    node_mappings: FxHashMap<SyntaxNode, MappingEntry>,
}

/// Internal entry: which parent and at what child slot index.
#[derive(Debug, Clone, Copy)]
struct MappingEntry {
    /// Index into `entry_parents`.
    parent: u32,
    /// Child index within the parent node.
    child_slot: u32,
}

impl SyntaxMapping {
    /// Like [`SyntaxMapping::upmap_child`] but for syntax elements.
    pub fn upmap_child_element(
        &self,
        child: &SyntaxElement,
        input_ancestor: &SyntaxNode,
        output_ancestor: &SyntaxNode,
    ) -> Result<SyntaxElement, MissingMapping> {
        match child {
            SyntaxElement::Node(node) => {
                self.upmap_child(node, input_ancestor, output_ancestor).map(SyntaxElement::from)
            }
            SyntaxElement::Token(token) => {
                let upmap_parent =
                    self.upmap_child(&token.parent().unwrap(), input_ancestor, output_ancestor)?;
                let element = upmap_parent.children_with_tokens().nth(token.index()).unwrap();
                Ok(element)
            }
        }
    }

    /// Maps a child node of the input ancestor to the corresponding node in
    /// the output ancestor.
    /// Algorithm: collect index path from child up to input_ancestor,
    /// then walk the same index path down from output_ancestor.
    pub fn upmap_child(
        &self,
        child: &SyntaxNode,
        input_ancestor: &SyntaxNode,
        output_ancestor: &SyntaxNode,
    ) -> Result<SyntaxNode, MissingMapping> {
        // Build index path from child up to input_ancestor
        let to_first_upmap = if child != input_ancestor {
            std::iter::successors(Some((child.index(), child.clone())), |(_, current)| {
                let parent = current.parent()?;
                if &parent == input_ancestor {
                    return None;
                }
                Some((parent.index(), parent))
            })
            .map(|(i, _)| i)
            .collect::<Vec<_>>()
        } else {
            vec![]
        };

        // Progressively up-map the input ancestor until we get to the output ancestor
        let to_output_ancestor = if input_ancestor != output_ancestor {
            self.upmap_to_ancestor(input_ancestor, output_ancestor)?
        } else {
            vec![]
        };

        // Walk down from output_ancestor using collected indices
        let to_map_down =
            to_output_ancestor.into_iter().rev().chain(to_first_upmap.into_iter().rev());

        let mut target = output_ancestor.clone();
        for index in to_map_down {
            target = target
                .children_with_tokens()
                .nth(index)
                .and_then(|it| it.into_node())
                .ok_or_else(|| MissingMapping(target.clone()))?;
        }

        Ok(target)
    }

    /// Follow the mapping chain from input_ancestor up to output_ancestor.
    fn upmap_to_ancestor(
        &self,
        input_ancestor: &SyntaxNode,
        output_ancestor: &SyntaxNode,
    ) -> Result<Vec<usize>, MissingMapping> {
        let mut current =
            self.upmap_node_single(input_ancestor).unwrap_or_else(|| input_ancestor.clone());
        let mut upmap_chain = vec![current.index()];

        loop {
            let Some(parent) = current.parent() else {
                break;
            };

            if &parent == output_ancestor {
                return Ok(upmap_chain);
            }

            current = match self.upmap_node_single(&parent) {
                Some(next) => next,
                None => parent,
            };
            upmap_chain.push(current.index());
        }

        Err(MissingMapping(current))
    }

    /// Map an element through the mapping tree to a position in the output root.
    ///
    /// Returns `None` if no mapping applies (element is at same position).
    /// Returns `Some(Ok(...))` if mapped successfully.
    /// Returns `Some(Err(...))` if mapping failed.
    pub fn upmap_element(
        &self,
        input: &SyntaxElement,
        output_root: &SyntaxNode,
    ) -> Option<Result<SyntaxElement, MissingMapping>> {
        match input {
            SyntaxElement::Node(node) => {
                Some(self.upmap_node(node, output_root)?.map(SyntaxElement::from))
            }
            SyntaxElement::Token(token) => {
                let upmap_parent = match self.upmap_node(&token.parent().unwrap(), output_root)? {
                    Ok(it) => it,
                    Err(err) => return Some(Err(err)),
                };
                let element = upmap_parent.children_with_tokens().nth(token.index()).unwrap();
                Some(Ok(element))
            }
        }
    }

    /// Map a node through the mapping tree to a position in the output root.
    pub fn upmap_node(
        &self,
        input: &SyntaxNode,
        output_root: &SyntaxNode,
    ) -> Option<Result<SyntaxNode, MissingMapping>> {
        let input_mapping = self.upmap_node_single(input);
        let input_ancestor =
            input.ancestors().find(|ancestor| self.upmap_node_single(ancestor).is_some());

        match (input_mapping, input_ancestor) {
            (Some(input_mapping), _) => {
                Some(self.upmap_child(&input_mapping, &input_mapping, output_root))
            }
            (None, Some(input_ancestor)) => {
                Some(self.upmap_child(input, &input_ancestor, output_root))
            }
            (None, None) => None,
        }
    }

    /// Merge another mapping into this one.
    pub fn merge(&mut self, other: SyntaxMapping) {
        let remap_base: u32 = self.entry_parents.len() as u32;
        self.entry_parents.extend(other.entry_parents);
        self.node_mappings.extend(other.node_mappings.into_iter().map(|(node, entry)| {
            (node, MappingEntry { parent: entry.parent + remap_base, ..entry })
        }));
    }

    /// Register a completed mapping builder.
    ///
    /// Register a completed mapping builder.
    pub fn add_mapping(&mut self, builder: SyntaxMappingBuilder) {
        let SyntaxMappingBuilder { parent_node, node_mappings } = builder;
        let parent_entry: u32 = self.entry_parents.len() as u32;
        self.entry_parents.push(parent_node);

        let entries = node_mappings
            .into_iter()
            .map(|(node, slot)| (node, MappingEntry { parent: parent_entry, child_slot: slot }));
        self.node_mappings.extend(entries);
    }

    /// Follow the input one step along the syntax mapping tree.
    fn upmap_node_single(&self, input: &SyntaxNode) -> Option<SyntaxNode> {
        let MappingEntry { parent, child_slot } = self.node_mappings.get(input)?;

        let output = self.entry_parents[*parent as usize]
            .children_with_tokens()
            .nth(*child_slot as usize)
            .and_then(|el| el.into_node())
            .unwrap();

        debug_assert_eq!(input.kind(), output.kind());
        Some(output)
    }

    /// Whether the mapping is empty.
    pub fn is_empty(&self) -> bool {
        self.node_mappings.is_empty()
    }
}

/// Builder for constructing a `SyntaxMapping`.
#[derive(Debug)]
pub struct SyntaxMappingBuilder {
    parent_node: SyntaxNode,
    node_mappings: Vec<(SyntaxNode, u32)>,
}

impl SyntaxMappingBuilder {
    /// Create a new builder for mappings whose output nodes are children of `parent_node`.
    pub fn new(parent_node: SyntaxNode) -> Self {
        Self { parent_node, node_mappings: vec![] }
    }

    /// Record a mapping: `input` in the original tree → `output` (child of parent_node).
    pub fn map_node(&mut self, input: SyntaxNode, output: SyntaxNode) {
        debug_assert_eq!(output.parent().as_ref(), Some(&self.parent_node));
        self.node_mappings.push((input, output.index() as u32));
    }

    /// Map children pairwise from input to output.
    pub fn map_children(
        &mut self,
        input: impl IntoIterator<Item = SyntaxNode>,
        output: impl IntoIterator<Item = SyntaxNode>,
    ) {
        for (inp, out) in input.into_iter().zip(output) {
            self.map_node(inp, out);
        }
    }

    /// Consume the builder and register it with a `SyntaxMapping`.
    pub fn finish(self, mappings: &mut SyntaxMapping) {
        mappings.add_mapping(self);
    }
}

/// Sentinel: a mapping was expected but not found.
#[derive(Debug)]
pub struct MissingMapping(pub SyntaxNode);
