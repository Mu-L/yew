//! This module contains the bundle implementation of a tag [BTag]

mod attributes;
mod listeners;

use std::cell::RefCell;
use std::collections::HashMap;
use std::hint::unreachable_unchecked;
use std::ops::DerefMut;

use gloo::utils::document;
use listeners::ListenerRegistration;
pub use listeners::Registry;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlTextAreaElement as TextAreaElement};

use super::{BNode, BSubtree, DomSlot, Reconcilable, ReconcileTarget};
use crate::html::AnyScope;
use crate::virtual_dom::vtag::{
    InputFields, TextareaFields, VTagInner, Value, MATHML_NAMESPACE, SVG_NAMESPACE,
};
use crate::virtual_dom::{AttrValue, Attributes, Key, VTag};
use crate::NodeRef;

/// Applies contained changes to DOM [web_sys::Element]
trait Apply {
    /// [web_sys::Element] subtype to apply the changes to
    type Element;
    type Bundle;

    /// Apply contained values to [Element](Self::Element) with no ancestor
    fn apply(self, root: &BSubtree, el: &Self::Element) -> Self::Bundle;

    /// Apply diff between [self] and `bundle` to [Element](Self::Element).
    fn apply_diff(self, root: &BSubtree, el: &Self::Element, bundle: &mut Self::Bundle);
}

/// [BTag] fields that are specific to different [BTag] kinds.
/// Decreases the memory footprint of [BTag] by avoiding impossible field and value combinations.
#[derive(Debug)]
enum BTagInner {
    /// Fields specific to
    /// [InputElement](https://developer.mozilla.org/en-US/docs/Web/HTML/Element/input)
    Input(InputFields),
    /// Fields specific to
    /// [TextArea](https://developer.mozilla.org/en-US/docs/Web/HTML/Element/textarea)
    Textarea {
        /// Contains a value of an
        /// [TextArea](https://developer.mozilla.org/en-US/docs/Web/HTML/Element/textarea)
        value: Value<TextAreaElement>,
    },
    /// Fields for all other kinds of [VTag]s
    Other {
        /// A tag of the element.
        tag: AttrValue,
        /// Child node.
        child_bundle: BNode,
    },
}

/// The bundle implementation to [VTag]
#[derive(Debug)]
pub(super) struct BTag {
    /// [BTag] fields that are specific to different [BTag] kinds.
    inner: BTagInner,
    listeners: ListenerRegistration,
    attributes: Attributes,
    /// A reference to the DOM [`Element`].
    reference: Element,
    /// A node reference used for DOM access in Component lifecycle methods
    node_ref: NodeRef,
    key: Option<Key>,
}

impl ReconcileTarget for BTag {
    fn detach(self, root: &BSubtree, parent: &Element, parent_to_detach: bool) {
        self.listeners.unregister(root);

        let node = self.reference;
        // recursively remove its children
        if let BTagInner::Other { child_bundle, .. } = self.inner {
            // This tag will be removed, so there's no point to remove any child.
            child_bundle.detach(root, &node, true);
        }
        if !parent_to_detach {
            let result = parent.remove_child(&node);

            if result.is_err() {
                tracing::warn!("Node not found to remove VTag");
            }
        }
        // It could be that the ref was already reused when rendering another element.
        // Only unset the ref it still belongs to our node
        if self.node_ref.get().as_ref() == Some(&node) {
            self.node_ref.set(None);
        }
    }

    fn shift(&self, next_parent: &Element, slot: DomSlot) -> DomSlot {
        slot.insert(next_parent, &self.reference);

        DomSlot::at(self.reference.clone().into())
    }
}

impl Reconcilable for VTag {
    type Bundle = BTag;

    fn attach(
        self,
        root: &BSubtree,
        parent_scope: &AnyScope,
        parent: &Element,
        slot: DomSlot,
    ) -> (DomSlot, Self::Bundle) {
        let el = self.create_element(parent);
        let Self {
            listeners,
            attributes,
            node_ref,
            key,
            ..
        } = self;
        slot.insert(parent, &el);

        let attributes = attributes.apply(root, &el);
        let listeners = listeners.apply(root, &el);

        let inner = match self.inner {
            VTagInner::Input(f) => {
                let f = f.apply(root, el.unchecked_ref());
                BTagInner::Input(f)
            }
            VTagInner::Textarea(f) => {
                let value = f.apply(root, el.unchecked_ref());
                BTagInner::Textarea { value }
            }
            VTagInner::Other { children, tag } => {
                let (_, child_bundle) = children.attach(root, parent_scope, &el, DomSlot::at_end());
                BTagInner::Other { child_bundle, tag }
            }
        };
        node_ref.set(Some(el.clone().into()));
        (
            DomSlot::at(el.clone().into()),
            BTag {
                inner,
                listeners,
                reference: el,
                attributes,
                key,
                node_ref,
            },
        )
    }

    fn reconcile_node(
        self,
        root: &BSubtree,
        parent_scope: &AnyScope,
        parent: &Element,
        slot: DomSlot,
        bundle: &mut BNode,
    ) -> DomSlot {
        // This kind of branching patching routine reduces branch predictor misses and the need to
        // unpack the enums (including `Option`s) all the time, resulting in a more streamlined
        // patching flow
        match bundle {
            // If the ancestor is a tag of the same type, don't recreate, keep the
            // old tag and update its attributes and children.
            BNode::Tag(ex) if self.key == ex.key => {
                if match (&self.inner, &ex.inner) {
                    (VTagInner::Input(_), BTagInner::Input(_)) => true,
                    (VTagInner::Textarea { .. }, BTagInner::Textarea { .. }) => true,
                    (VTagInner::Other { tag: l, .. }, BTagInner::Other { tag: r, .. })
                        if l == r =>
                    {
                        true
                    }
                    _ => false,
                } {
                    return self.reconcile(root, parent_scope, parent, slot, ex.deref_mut());
                }
            }
            _ => {}
        };
        self.replace(root, parent_scope, parent, slot, bundle)
    }

    fn reconcile(
        self,
        root: &BSubtree,
        parent_scope: &AnyScope,
        _parent: &Element,
        _slot: DomSlot,
        tag: &mut Self::Bundle,
    ) -> DomSlot {
        let el = &tag.reference;
        self.attributes.apply_diff(root, el, &mut tag.attributes);
        self.listeners.apply_diff(root, el, &mut tag.listeners);

        match (self.inner, &mut tag.inner) {
            (VTagInner::Input(new), BTagInner::Input(old)) => {
                new.apply_diff(root, el.unchecked_ref(), old);
            }
            (
                VTagInner::Textarea(TextareaFields { value: new, .. }),
                BTagInner::Textarea { value: old },
            ) => {
                new.apply_diff(root, el.unchecked_ref(), old);
            }
            (
                VTagInner::Other { children: new, .. },
                BTagInner::Other {
                    child_bundle: old, ..
                },
            ) => {
                new.reconcile(root, parent_scope, el, DomSlot::at_end(), old);
            }
            // Can not happen, because we checked for tag equability above
            _ => unsafe { unreachable_unchecked() },
        }

        tag.key = self.key;

        if self.node_ref != tag.node_ref && tag.node_ref.get().as_ref() == Some(el) {
            tag.node_ref.set(None);
        }
        if self.node_ref != tag.node_ref {
            tag.node_ref = self.node_ref;
            tag.node_ref.set(Some(el.clone().into()));
        }

        DomSlot::at(el.clone().into())
    }
}

impl VTag {
    fn create_element(&self, parent: &Element) -> Element {
        let tag = self.tag();
        // check for an xmlns attribute. If it exists, create an element with the specified
        // namespace
        if let Some(xmlns) = self
            .attributes
            .iter()
            .find(|(k, _)| *k == "xmlns")
            .map(|(_, v)| v)
        {
            document()
                .create_element_ns(Some(xmlns), tag)
                .expect("can't create namespaced element for vtag")
        } else if tag == "svg" || parent.namespace_uri().is_some_and(|ns| ns == SVG_NAMESPACE) {
            let namespace = Some(SVG_NAMESPACE);
            document()
                .create_element_ns(namespace, tag)
                .expect("can't create namespaced element for vtag")
        } else if tag == "math"
            || parent
                .namespace_uri()
                .is_some_and(|ns| ns == MATHML_NAMESPACE)
        {
            let namespace = Some(MATHML_NAMESPACE);
            document()
                .create_element_ns(namespace, tag)
                .expect("can't create namespaced element for vtag")
        } else {
            thread_local! {
                static CACHED_ELEMENTS: RefCell<HashMap<String, Element>> = RefCell::new(HashMap::with_capacity(32));
            }

            CACHED_ELEMENTS.with(|cache| {
                let mut cache = cache.borrow_mut();
                let cached = cache.get(tag).map(|el| {
                    el.clone_node()
                        .expect("couldn't clone cached element")
                        .unchecked_into::<Element>()
                });
                cached.unwrap_or_else(|| {
                    let to_be_cached = document()
                        .create_element(tag)
                        .expect("can't create element for vtag");
                    cache.insert(
                        tag.to_string(),
                        to_be_cached
                            .clone_node()
                            .expect("couldn't clone node to be cached")
                            .unchecked_into(),
                    );
                    to_be_cached
                })
            })
        }
    }
}

impl BTag {
    /// Get the key of the underlying tag
    pub fn key(&self) -> Option<&Key> {
        self.key.as_ref()
    }

    #[cfg(all(target_arch = "wasm32", not(target_os = "wasi")))]
    #[cfg(test)]
    fn reference(&self) -> &Element {
        &self.reference
    }

    #[cfg(all(target_arch = "wasm32", not(target_os = "wasi")))]
    #[cfg(test)]
    fn children(&self) -> Option<&BNode> {
        match &self.inner {
            BTagInner::Other { child_bundle, .. } => Some(child_bundle),
            _ => None,
        }
    }

    #[cfg(all(target_arch = "wasm32", not(target_os = "wasi")))]
    #[cfg(test)]
    fn tag(&self) -> &str {
        match &self.inner {
            BTagInner::Input { .. } => "input",
            BTagInner::Textarea { .. } => "textarea",
            BTagInner::Other { tag, .. } => tag.as_ref(),
        }
    }
}

#[cfg(feature = "hydration")]
mod feat_hydration {
    use web_sys::Node;

    use super::*;
    use crate::dom_bundle::{node_type_str, Fragment, Hydratable};

    impl Hydratable for VTag {
        fn hydrate(
            self,
            root: &BSubtree,
            parent_scope: &AnyScope,
            _parent: &Element,
            fragment: &mut Fragment,
        ) -> Self::Bundle {
            let tag_name = self.tag().to_owned();

            let Self {
                inner,
                listeners,
                attributes,
                node_ref,
                key,
            } = self;

            // We trim all text nodes as it's likely these are whitespaces.
            fragment.trim_start_text_nodes();

            let node = fragment
                .pop_front()
                .unwrap_or_else(|| panic!("expected element of type {tag_name}, found EOF."));

            assert_eq!(
                node.node_type(),
                Node::ELEMENT_NODE,
                "expected element, found node type {}.",
                node_type_str(&node),
            );
            let el = node.dyn_into::<Element>().expect("expected an element.");

            assert_eq!(
                el.tag_name().to_lowercase(),
                tag_name,
                "expected element of kind {}, found {}.",
                tag_name,
                el.tag_name().to_lowercase(),
            );

            // We simply register listeners and update all attributes.
            let attributes = attributes.apply(root, &el);
            let listeners = listeners.apply(root, &el);

            // For input and textarea elements, we update their value anyways.
            let inner = match inner {
                VTagInner::Input(f) => {
                    let f = f.apply(root, el.unchecked_ref());
                    BTagInner::Input(f)
                }
                VTagInner::Textarea(f) => {
                    let value = f.apply(root, el.unchecked_ref());

                    BTagInner::Textarea { value }
                }
                VTagInner::Other { children, tag } => {
                    let mut nodes = Fragment::collect_children(&el);
                    let child_bundle = children.hydrate(root, parent_scope, &el, &mut nodes);

                    nodes.trim_start_text_nodes();

                    assert!(nodes.is_empty(), "expected EOF, found node.");

                    BTagInner::Other { child_bundle, tag }
                }
            };

            node_ref.set(Some((*el).clone()));

            BTag {
                inner,
                listeners,
                attributes,
                reference: el,
                node_ref,
                key,
            }
        }
    }
}

#[cfg(all(target_arch = "wasm32", not(target_os = "wasi")))]
#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use wasm_bindgen::JsCast;
    use wasm_bindgen_test::{wasm_bindgen_test as test, wasm_bindgen_test_configure};
    use web_sys::HtmlInputElement as InputElement;

    use super::*;
    use crate::dom_bundle::utils::setup_parent;
    use crate::dom_bundle::{BNode, Reconcilable, ReconcileTarget};
    use crate::utils::RcExt;
    use crate::virtual_dom::vtag::{HTML_NAMESPACE, SVG_NAMESPACE};
    use crate::virtual_dom::{AttrValue, VNode, VTag};
    use crate::{html, Html, NodeRef};

    wasm_bindgen_test_configure!(run_in_browser);

    #[test]
    fn it_compares_tags() {
        let a = html! {
            <div></div>
        };

        let b = html! {
            <div></div>
        };

        let c = html! {
            <p></p>
        };

        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn it_compares_text() {
        let a = html! {
            <div>{ "correct" }</div>
        };

        let b = html! {
            <div>{ "correct" }</div>
        };

        let c = html! {
            <div>{ "incorrect" }</div>
        };

        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn it_compares_attributes_static() {
        let a = html! {
            <div a="test"></div>
        };

        let b = html! {
            <div a="test"></div>
        };

        let c = html! {
            <div a="fail"></div>
        };

        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn it_compares_attributes_dynamic() {
        let a = html! {
            <div a={"test".to_owned()}></div>
        };

        let b = html! {
            <div a={"test".to_owned()}></div>
        };

        let c = html! {
            <div a={"fail".to_owned()}></div>
        };

        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn it_compares_children() {
        let a = html! {
            <div>
                <p></p>
            </div>
        };

        let b = html! {
            <div>
                <p></p>
            </div>
        };

        let c = html! {
            <div>
                <span></span>
            </div>
        };

        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn it_compares_classes_static() {
        let a = html! {
            <div class="test"></div>
        };

        let b = html! {
            <div class="test"></div>
        };

        let c = html! {
            <div class="fail"></div>
        };

        let d = html! {
            <div class={format!("fail{}", "")}></div>
        };

        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }

    #[test]
    fn it_compares_classes_dynamic() {
        let a = html! {
            <div class={"test".to_owned()}></div>
        };

        let b = html! {
            <div class={"test".to_owned()}></div>
        };

        let c = html! {
            <div class={"fail".to_owned()}></div>
        };

        let d = html! {
            <div class={format!("fail{}", "")}></div>
        };

        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }

    fn assert_vtag(node: VNode) -> VTag {
        if let VNode::VTag(vtag) = node {
            return RcExt::unwrap_or_clone(vtag);
        }
        panic!("should be vtag");
    }

    fn assert_btag_ref(node: &BNode) -> &BTag {
        if let BNode::Tag(vtag) = node {
            return vtag;
        }
        panic!("should be btag");
    }

    fn assert_vtag_ref(node: &VNode) -> &VTag {
        if let VNode::VTag(vtag) = node {
            return vtag;
        }
        panic!("should be vtag");
    }

    fn assert_btag_mut(node: &mut BNode) -> &mut BTag {
        if let BNode::Tag(btag) = node {
            return btag;
        }
        panic!("should be btag");
    }

    fn assert_namespace(vtag: &BTag, namespace: &'static str) {
        assert_eq!(vtag.reference().namespace_uri().unwrap(), namespace);
    }

    #[test]
    fn supports_svg() {
        let (root, scope, parent) = setup_parent();
        let document = web_sys::window().unwrap().document().unwrap();

        let namespace = SVG_NAMESPACE;
        let namespace = Some(namespace);
        let svg_el = document.create_element_ns(namespace, "svg").unwrap();

        let g_node = html! { <g class="segment"></g> };
        let path_node = html! { <path></path> };
        let svg_node = html! { <svg>{path_node}</svg> };

        let svg_tag = assert_vtag(svg_node);
        let (_, svg_tag) = svg_tag.attach(&root, &scope, &parent, DomSlot::at_end());
        assert_namespace(&svg_tag, SVG_NAMESPACE);
        let path_tag = assert_btag_ref(svg_tag.children().unwrap());
        assert_namespace(path_tag, SVG_NAMESPACE);

        let g_tag = assert_vtag(g_node.clone());
        let (_, g_tag) = g_tag.attach(&root, &scope, &parent, DomSlot::at_end());
        assert_namespace(&g_tag, HTML_NAMESPACE);

        let g_tag = assert_vtag(g_node);
        let (_, g_tag) = g_tag.attach(&root, &scope, &svg_el, DomSlot::at_end());
        assert_namespace(&g_tag, SVG_NAMESPACE);
    }

    #[test]
    fn supports_mathml() {
        let (root, scope, parent) = setup_parent();
        let mfrac_node = html! { <mfrac> </mfrac> };
        let math_node = html! { <math>{mfrac_node}</math> };

        let math_tag = assert_vtag(math_node);
        let (_, math_tag) = math_tag.attach(&root, &scope, &parent, DomSlot::at_end());
        assert_namespace(&math_tag, MATHML_NAMESPACE);
        let mfrac_tag = assert_btag_ref(math_tag.children().unwrap());
        assert_namespace(mfrac_tag, MATHML_NAMESPACE);
    }

    #[test]
    fn it_compares_values() {
        let a = html! {
            <input value="test"/>
        };

        let b = html! {
            <input value="test"/>
        };

        let c = html! {
            <input value="fail"/>
        };

        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn it_compares_kinds() {
        let a = html! {
            <input type="text"/>
        };

        let b = html! {
            <input type="text"/>
        };

        let c = html! {
            <input type="hidden"/>
        };

        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn it_compares_checked() {
        let a = html! {
            <input type="checkbox" checked=false />
        };

        let b = html! {
            <input type="checkbox" checked=false />
        };

        let c = html! {
            <input type="checkbox" checked=true />
        };

        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn it_allows_aria_attributes() {
        let a = html! {
            <p aria-controls="it-works">
                <a class="btn btn-primary"
                   data-toggle="collapse"
                   href="#collapseExample"
                   role="button"
                   aria-expanded="false"
                   aria-controls="collapseExample">
                    { "Link with href" }
                </a>
                <button class="btn btn-primary"
                        type="button"
                        data-toggle="collapse"
                        data-target="#collapseExample"
                        aria-expanded="false"
                        aria-controls="collapseExample">
                    { "Button with data-target" }
                </button>
                <div own-attribute-with-multiple-parts="works" />
            </p>
        };
        if let VNode::VTag(vtag) = a {
            assert_eq!(
                vtag.attributes
                    .iter()
                    .find(|(k, _)| k == &"aria-controls")
                    .map(|(_, v)| v),
                Some("it-works")
            );
        } else {
            panic!("vtag expected");
        }
    }

    #[test]
    fn it_does_not_set_missing_class_name() {
        let (root, scope, parent) = setup_parent();

        let elem = html! { <div></div> };
        let (_, mut elem) = elem.attach(&root, &scope, &parent, DomSlot::at_end());
        let vtag = assert_btag_mut(&mut elem);
        // test if the className has not been set
        assert!(!vtag.reference().has_attribute("class"));
    }

    fn test_set_class_name(gen_html: impl FnOnce() -> Html) {
        let (root, scope, parent) = setup_parent();

        let elem = gen_html();
        let (_, mut elem) = elem.attach(&root, &scope, &parent, DomSlot::at_end());
        let vtag = assert_btag_mut(&mut elem);
        // test if the className has been set
        assert!(vtag.reference().has_attribute("class"));
    }

    #[test]
    fn it_sets_class_name_static() {
        test_set_class_name(|| html! { <div class="ferris the crab"></div> });
    }

    #[test]
    fn it_sets_class_name_dynamic() {
        test_set_class_name(|| html! { <div class={"ferris the crab".to_owned()}></div> });
    }

    #[test]
    fn controlled_input_synced() {
        let (root, scope, parent) = setup_parent();

        let expected = "not_changed_value";

        // Initial state
        let elem = html! { <input value={expected} /> };
        let (_, mut elem) = elem.attach(&root, &scope, &parent, DomSlot::at_end());
        let vtag = assert_btag_ref(&elem);

        // User input
        let input_ref = &vtag.reference();
        let input = input_ref.dyn_ref::<InputElement>();
        input.unwrap().set_value("User input");

        let next_elem = html! { <input value={expected} /> };
        let elem_vtag = assert_vtag(next_elem);

        // Sync happens here
        elem_vtag.reconcile_node(&root, &scope, &parent, DomSlot::at_end(), &mut elem);
        let vtag = assert_btag_ref(&elem);

        // Get new current value of the input element
        let input_ref = &vtag.reference();
        let input = input_ref.dyn_ref::<InputElement>().unwrap();

        let current_value = input.value();

        // check whether not changed virtual dom value has been set to the input element
        assert_eq!(current_value, expected);
    }

    #[test]
    fn uncontrolled_input_unsynced() {
        let (root, scope, parent) = setup_parent();

        // Initial state
        let elem = html! { <input /> };
        let (_, mut elem) = elem.attach(&root, &scope, &parent, DomSlot::at_end());
        let vtag = assert_btag_ref(&elem);

        // User input
        let input_ref = &vtag.reference();
        let input = input_ref.dyn_ref::<InputElement>();
        input.unwrap().set_value("User input");

        let next_elem = html! { <input /> };
        let elem_vtag = assert_vtag(next_elem);

        // Value should not be refreshed
        elem_vtag.reconcile_node(&root, &scope, &parent, DomSlot::at_end(), &mut elem);
        let vtag = assert_btag_ref(&elem);

        // Get user value of the input element
        let input_ref = &vtag.reference();
        let input = input_ref.dyn_ref::<InputElement>().unwrap();

        let current_value = input.value();

        // check whether not changed virtual dom value has been set to the input element
        assert_eq!(current_value, "User input");

        // Need to remove the element to clean up the dirty state of the DOM. Failing this causes
        // event listener tests to fail.
        parent.remove();
    }

    #[test]
    fn dynamic_tags_work() {
        let (root, scope, parent) = setup_parent();

        let elem = html! { <@{{
            let mut builder = String::new();
            builder.push('a');
            builder
        }}/> };

        let (_, mut elem) = elem.attach(&root, &scope, &parent, DomSlot::at_end());
        let vtag = assert_btag_mut(&mut elem);
        // make sure the new tag name is used internally
        assert_eq!(vtag.tag(), "a");

        // Element.tagName is always in the canonical upper-case form.
        assert_eq!(vtag.reference().tag_name(), "A");
    }

    #[test]
    fn dynamic_tags_handle_value_attribute() {
        let div_el = html! {
            <@{"div"} value="Hello"/>
        };
        let div_vtag = assert_vtag_ref(&div_el);
        assert!(div_vtag.value().is_none());
        let v: Option<&str> = div_vtag
            .attributes
            .iter()
            .find(|(k, _)| k == &"value")
            .map(|(_, v)| AsRef::as_ref(v));
        assert_eq!(v, Some("Hello"));

        let input_el = html! {
            <@{"input"} value="World"/>
        };
        let input_vtag = assert_vtag_ref(&input_el);
        assert_eq!(input_vtag.value(), Some(&AttrValue::Static("World")));
        assert!(!input_vtag.attributes.iter().any(|(k, _)| k == "value"));
    }

    #[test]
    fn dynamic_tags_handle_weird_capitalization() {
        let el = html! {
            <@{"tExTAREa"}/>
        };
        let vtag = assert_vtag_ref(&el);
        // textarea is a special element, so it gets normalized
        assert_eq!(vtag.tag(), "textarea");
    }

    #[test]
    fn dynamic_tags_allow_custom_capitalization() {
        let el = html! {
            <@{"clipPath"}/>
        };
        let vtag = assert_vtag_ref(&el);
        // no special treatment for elements not recognized e.g. clipPath
        assert_eq!(vtag.tag(), "clipPath");
    }

    #[test]
    fn reset_node_ref() {
        let (root, scope, parent) = setup_parent();

        let node_ref = NodeRef::default();
        let elem: VNode = html! { <div ref={node_ref.clone()}></div> };
        assert_vtag_ref(&elem);
        let (_, elem) = elem.attach(&root, &scope, &parent, DomSlot::at_end());
        assert_eq!(node_ref.get(), parent.first_child());
        elem.detach(&root, &parent, false);
        assert!(node_ref.get().is_none());
    }

    #[test]
    fn vtag_reuse_should_reset_ancestors_node_ref() {
        let (root, scope, parent) = setup_parent();

        let node_ref_a = NodeRef::default();
        let elem_a = html! { <div id="a" ref={node_ref_a.clone()} /> };
        let (_, mut elem) = elem_a.attach(&root, &scope, &parent, DomSlot::at_end());

        // save the Node to check later that it has been reused.
        let node_a = node_ref_a.get().unwrap();

        let node_ref_b = NodeRef::default();
        let elem_b = html! { <div id="b" ref={node_ref_b.clone()} /> };
        elem_b.reconcile_node(&root, &scope, &parent, DomSlot::at_end(), &mut elem);

        let node_b = node_ref_b.get().unwrap();

        assert_eq!(node_a, node_b, "VTag should have reused the element");
        assert!(
            node_ref_a.get().is_none(),
            "node_ref_a should have been reset when the element was reused."
        );
    }

    #[test]
    fn vtag_should_not_touch_newly_bound_refs() {
        let (root, scope, parent) = setup_parent();

        let test_ref = NodeRef::default();
        let before = html! {
            <>
                <div ref={&test_ref} id="before" />
            </>
        };
        let after = html! {
            <>
                <h6 />
                <div ref={&test_ref} id="after" />
            </>
        };
        // The point of this diff is to first render the "after" div and then detach the "before"
        // div, while both should be bound to the same node ref

        let (_, mut elem) = before.attach(&root, &scope, &parent, DomSlot::at_end());
        after.reconcile_node(&root, &scope, &parent, DomSlot::at_end(), &mut elem);

        assert_eq!(
            test_ref
                .get()
                .unwrap()
                .dyn_ref::<web_sys::Element>()
                .unwrap()
                .outer_html(),
            "<div id=\"after\"></div>"
        );
    }

    // test for bug: https://github.com/yewstack/yew/pull/2653
    #[test]
    fn test_index_map_attribute_diff() {
        let (root, scope, parent) = setup_parent();

        let test_ref = NodeRef::default();

        // We want to test appy_diff with Attributes::IndexMap, so we
        // need to create the VTag manually

        // Create <div disabled="disabled" tabindex="0">
        let mut vtag = VTag::new("div");
        vtag.node_ref = test_ref.clone();
        vtag.add_attribute("disabled", "disabled");
        vtag.add_attribute("tabindex", "0");

        let elem = VNode::VTag(Rc::new(vtag));

        let (_, mut elem) = elem.attach(&root, &scope, &parent, DomSlot::at_end());

        // Create <div tabindex="0"> (removed first attribute "disabled")
        let mut vtag = VTag::new("div");
        vtag.node_ref = test_ref.clone();
        vtag.add_attribute("tabindex", "0");
        let next_elem = VNode::VTag(Rc::new(vtag));
        let elem_vtag = assert_vtag(next_elem);

        // Sync happens here
        // this should remove the "disabled" attribute
        elem_vtag.reconcile_node(&root, &scope, &parent, DomSlot::at_end(), &mut elem);

        assert_eq!(
            test_ref
                .get()
                .unwrap()
                .dyn_ref::<web_sys::Element>()
                .unwrap()
                .outer_html(),
            "<div tabindex=\"0\"></div>"
        );
    }
}

#[cfg(all(target_arch = "wasm32", not(target_os = "wasi")))]
#[cfg(test)]
mod layout_tests {
    extern crate self as yew;

    use wasm_bindgen_test::{wasm_bindgen_test as test, wasm_bindgen_test_configure};

    use crate::html;
    use crate::tests::layout_tests::{diff_layouts, TestLayout};

    wasm_bindgen_test_configure!(run_in_browser);

    #[test]
    fn diff() {
        let layout1 = TestLayout {
            name: "1",
            node: html! {
                <ul>
                    <li>
                        {"a"}
                    </li>
                    <li>
                        {"b"}
                    </li>
                </ul>
            },
            expected: "<ul><li>a</li><li>b</li></ul>",
        };

        let layout2 = TestLayout {
            name: "2",
            node: html! {
                <ul>
                    <li>
                        {"a"}
                    </li>
                    <li>
                        {"b"}
                    </li>
                    <li>
                        {"d"}
                    </li>
                </ul>
            },
            expected: "<ul><li>a</li><li>b</li><li>d</li></ul>",
        };

        let layout3 = TestLayout {
            name: "3",
            node: html! {
                <ul>
                    <li>
                        {"a"}
                    </li>
                    <li>
                        {"b"}
                    </li>
                    <li>
                        {"c"}
                    </li>
                    <li>
                        {"d"}
                    </li>
                </ul>
            },
            expected: "<ul><li>a</li><li>b</li><li>c</li><li>d</li></ul>",
        };

        let layout4 = TestLayout {
            name: "4",
            node: html! {
                <ul>
                    <li>
                        <>
                            {"a"}
                        </>
                    </li>
                    <li>
                        {"b"}
                        <li>
                            {"c"}
                        </li>
                        <li>
                            {"d"}
                        </li>
                    </li>
                </ul>
            },
            expected: "<ul><li>a</li><li>b<li>c</li><li>d</li></li></ul>",
        };

        diff_layouts(vec![layout1, layout2, layout3, layout4]);
    }
}

#[cfg(test)]
mod tests_without_browser {
    use crate::html;
    use crate::virtual_dom::VNode;

    #[test]
    fn html_if_bool() {
        assert_eq!(
            html! {
                if true {
                    <div class="foo" />
                }
            },
            html! {
                <>
                    <div class="foo" />
                </>
            },
        );
        assert_eq!(
            html! {
                if false {
                    <div class="foo" />
                } else {
                    <div class="bar" />
                }
            },
            html! {
                <><div class="bar" /></>
            },
        );
        assert_eq!(
            html! {
                if false {
                    <div class="foo" />
                }
            },
            html! {
                <></>
            },
        );

        // non-root tests
        assert_eq!(
            html! {
                <div>
                    if true {
                        <div class="foo" />
                    }
                </div>
            },
            html! {
                <div>
                    <><div class="foo" /></>
                </div>
            },
        );
        assert_eq!(
            html! {
                <div>
                    if false {
                        <div class="foo" />
                    } else {
                        <div class="bar" />
                    }
                </div>
            },
            html! {
                <div>
                    <><div class="bar" /></>
                </div>
            },
        );
        assert_eq!(
            html! {
                <div>
                    if false {
                        <div class="foo" />
                    }
                </div>
            },
            html! {
                <div>
                    <></>
                </div>
            },
        );
    }

    #[test]
    fn html_if_option() {
        let option_foo = Some("foo");
        let none: Option<&'static str> = None;
        assert_eq!(
            html! {
                if let Some(class) = option_foo {
                    <div class={class} />
                }
            },
            html! {
                <>
                    <div class={Some("foo")} />
                </>
            },
        );
        assert_eq!(
            html! {
                if let Some(class) = none {
                    <div class={class} />
                } else {
                    <div class="bar" />
                }
            },
            html! {
                <>
                    <div class="bar" />
                </>
            },
        );
        assert_eq!(
            html! {
                if let Some(class) = none {
                    <div class={class} />
                }
            },
            html! {
                <></>
            },
        );

        // non-root tests
        assert_eq!(
            html! {
                <div>
                    if let Some(class) = option_foo {
                        <div class={class} />
                    }
                </div>
            },
            html! {
                <div>
                    <>
                        <div class={Some("foo")} />
                    </>
                </div>
            },
        );
        assert_eq!(
            html! {
                <div>
                    if let Some(class) = none {
                        <div class={class} />
                    } else {
                        <div class="bar" />
                    }
                </div>
            },
            html! {
                <div>
                    <>
                        <div class="bar" />
                    </>
                </div>
            },
        );
        assert_eq!(
            html! {
                <div>
                    if let Some(class) = none {
                        <div class={class} />
                    }
                </div>
            },
            html! { <div><></></div> },
        );
    }

    #[test]
    fn input_checked_stays_there() {
        let tag = html! {
            <input checked={true} />
        };
        match tag {
            VNode::VTag(tag) => {
                assert_eq!(tag.checked(), Some(true));
            }
            _ => unreachable!(),
        }
    }
    #[test]
    fn non_input_checked_stays_there() {
        let tag = html! {
            <my-el checked="true" />
        };
        match tag {
            VNode::VTag(tag) => {
                assert_eq!(
                    tag.attributes.iter().find(|(k, _)| *k == "checked"),
                    Some(("checked", "true"))
                );
            }
            _ => unreachable!(),
        }
    }
}
