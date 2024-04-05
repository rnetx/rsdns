use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    hash::Hash,
    rc::{Rc, Weak},
    sync::Arc,
};

use crate::adapter;

struct GraphNode<T: Eq + Hash> {
    data: T,
    prev: RefCell<HashMap<T, Weak<Self>>>,
    next: RefCell<HashMap<T, Weak<Self>>>,
}

impl<T: Clone + Eq + Hash> GraphNode<T> {
    fn new(data: T) -> Self {
        Self {
            data,
            prev: RefCell::new(HashMap::new()),
            next: RefCell::new(HashMap::new()),
        }
    }

    fn data(&self) -> &T {
        &self.data
    }

    fn add_prev(&self, node: Rc<Self>) {
        self.prev.borrow_mut().insert(node.data().clone(), Rc::downgrade(&node));
    }

    fn add_next(&self, node: Rc<Self>) {
        self.next.borrow_mut().insert(node.data().clone(), Rc::downgrade(&node));
    }

    fn remove_prev(&self, data_ref: &T) {
        self.prev.borrow_mut().remove(data_ref);
    }

    fn have_prev(&self) -> bool {
        self.prev.borrow().len() > 0
    }

    fn next_list(&self) -> Vec<T> {
        self.next.borrow().keys().cloned().collect()
    }
}

struct Graph<T: Eq + Hash>(HashMap<T, Rc<GraphNode<T>>>);

impl<T: Eq + Hash> Default for Graph<T> {
    fn default() -> Self {
        Self(HashMap::new())
    }
}

impl<T: Clone + Eq + Hash> Graph<T> {
    fn get_or_insert(&mut self, data: T) -> Rc<GraphNode<T>> {
        self.0
            .entry(data.clone())
            .or_insert_with(|| Rc::new(GraphNode::new(data)))
            .clone()
    }

    fn simple_remove(&mut self, data: &T) {
        self.0.remove(data);
    }

    fn add_link(&self, from: T, to: T) {
        match (self.0.get(&from), self.0.get(&to)) {
            (Some(from_node), Some(to_node)) => {
                from_node.add_next(to_node.clone());
                to_node.add_prev(from_node.clone());
            }
            _ => {}
        }
    }

    fn find_circle_link(&self) -> Vec<Rc<GraphNode<T>>> {
        for v in self.0.values() {
            let mut link = Vec::new();
            link.push(v.clone());
            for next in v.next_list() {
                let next_node = self.0.get(&next).unwrap().clone();
                link.push(next_node.clone());
                if self.find_circle_link_wrapper(&mut link, next_node) {
                    return link;
                }
                link.pop();
            }
        }
        Vec::new()
    }

    fn find_circle_link_wrapper(
        &self,
        link: &mut Vec<Rc<GraphNode<T>>>,
        now: Rc<GraphNode<T>>,
    ) -> bool {
        if link.first().unwrap().data() == now.data() {
            return true;
        }
        for next in now.next_list() {
            let next_node = self.0.get(&next).unwrap().clone();
            link.push(next_node.clone());
            if self.find_circle_link_wrapper(link, next_node) {
                return true;
            }
            link.pop();
        }
        false
    }
}

pub(super) fn upstream_topological_sort(
    list: &mut Vec<Arc<Box<dyn adapter::Upstream>>>,
) -> Result<(), String> {
    let upstream_map = list
        .iter()
        .map(|upstream| (upstream.tag().to_string(), upstream.clone()))
        .collect::<HashMap<_, _>>();
    let mut graph = Graph(HashMap::new());
    for item in list.iter() {
        let node = graph.get_or_insert(item.tag().to_string());
        if let Some(dependencies) = item.dependencies() {
            for dependency in dependencies {
                graph.get_or_insert(dependency.clone());
                graph.add_link(dependency.clone(), node.data().clone());
            }
        }
    }
    let mut new_list = Vec::with_capacity(list.len());
    let mut work_stack = VecDeque::new();
    for item in list.iter() {
        let node = graph.get_or_insert(item.tag().to_string());
        if !node.have_prev() {
            work_stack.push_back(node);
        }
    }
    if work_stack.is_empty() {
        let circle_link = graph
            .find_circle_link()
            .into_iter()
            .map(|node| format!("[{}]", node.data()))
            .collect::<Vec<_>>();
        return Err(format!("find upstream circle link: {}", circle_link.join(" -> ")).into());
    }
    loop {
        let node = match work_stack.pop_front() {
            Some(node) => node,
            None => break,
        };
        new_list.push(upstream_map[node.data()].clone());
        node.next_list().iter().for_each(|next| {
            let next_node = graph.get_or_insert(next.clone());
            next_node.remove_prev(node.data());
            if !next_node.have_prev() {
                work_stack.push_back(next_node);
            }
        });
        graph.simple_remove(node.data());
    }
    if list.len() != new_list.len() {
        let circle_link = graph
            .find_circle_link()
            .into_iter()
            .map(|node| format!("[{}]", node.data()))
            .collect::<Vec<_>>();
        return Err(format!("find upstream circle link: {}", circle_link.join(" -> ")).into());
    }
    *list = new_list;
    Ok(())
}
