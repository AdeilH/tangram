use super::Data;
use crate as tg;
use itertools::Itertools as _;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub enum File {
	Normal {
		contents: tg::Blob,
		dependencies: BTreeMap<tg::Reference, tg::Referent<tg::Object>>,
		executable: bool,
	},
	Graph {
		graph: tg::Graph,
		node: usize,
	},
}

impl File {
	#[must_use]
	pub fn children(&self) -> Vec<tg::Object> {
		match self {
			Self::Normal {
				contents,
				dependencies,
				..
			} => {
				let contents = contents.clone().into();
				let dependencies = dependencies
					.values()
					.map(|dependency| dependency.item.clone());
				std::iter::once(contents).chain(dependencies).collect()
			},
			Self::Graph { graph, .. } => [graph.clone()].into_iter().map_into().collect(),
		}
	}
}

impl TryFrom<Data> for File {
	type Error = tg::Error;

	fn try_from(data: Data) -> std::result::Result<Self, Self::Error> {
		match data {
			Data::Normal {
				contents,
				dependencies,
				executable,
			} => {
				let contents = tg::Blob::with_id(contents);
				let dependencies = dependencies
					.into_iter()
					.map(|(reference, referent)| {
						let referent = tg::Referent {
							item: tg::Object::with_id(referent.item),
							subpath: None,
							tag: referent.tag.clone(),
						};
						(reference, referent)
					})
					.collect();
				Ok(Self::Normal {
					contents,
					dependencies,
					executable,
				})
			},
			Data::Graph { graph, node } => {
				let graph = tg::Graph::with_id(graph);
				Ok(Self::Graph { graph, node })
			},
		}
	}
}