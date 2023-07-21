use crate::{
    graph::Graph,
    op::{self},
    shape::*,
    tensor::Tensor,
};
use std::{
    marker::PhantomData,
    ops::{Add, Div, Mul, Sub},
};

use itertools::Itertools;
use petgraph::graph::NodeIndex;

#[derive(Clone, Copy)]
pub struct GraphTensor<S: ConstShape> {
    pub id: NodeIndex,
    pub graph_ref: *mut Graph,
    pub(crate) _phantom: PhantomData<S>,
}

impl<S: ConstShape> GraphTensor<S> {
    fn from_id(id: NodeIndex, graph_ref: *mut Graph) -> Self {
        Self {
            id,
            graph_ref,
            _phantom: Default::default(),
        }
    }

    /// Mark this tensor to be retrieved later
    pub fn mark(&self) {
        unsafe { self.graph_ref.as_mut().unwrap().no_delete.insert(self.id) };
    }

    /// Get the value of the tensor (if the graph was executed)
    pub fn retrieve(self) -> Option<Tensor> {
        unsafe { self.graph_ref.as_mut().unwrap().get_tensor(self.id) }
    }

    /// Set the value of the tensor
    pub fn set(&self, data: Vec<f32>) {
        unsafe { self.graph_ref.as_mut().unwrap().set_tensor(*self, data) }
    }

    pub fn log_2(self) -> GraphTensor<S> {
        let graph = unsafe { &mut self.graph_ref.as_mut().unwrap().graph };
        let new_id = graph.add_node(Box::new(op::Log2));
        graph.add_edge(self.id, new_id, 0);
        GraphTensor::from_id(new_id, self.graph_ref)
    }

    pub fn exp_2(self) -> GraphTensor<S> {
        let graph = unsafe { &mut self.graph_ref.as_mut().unwrap().graph };
        let new_id = graph.add_node(Box::new(op::Exp2));
        graph.add_edge(self.id, new_id, 0);
        GraphTensor::from_id(new_id, self.graph_ref)
    }

    pub fn permute<N: ConstShape, Dst, Ax: Axes>(self) -> GraphTensor<N>
    where
        N: PermuteShapeTo<Dst, Ax>,
    {
        let graph = unsafe { &mut self.graph_ref.as_mut().unwrap().graph };
        let new_id = graph.add_node(Box::new(op::Permute(
            Ax::as_array().into_iter().map(|i| i as usize).collect_vec(),
        )));
        graph.add_edge(self.id, new_id, 0);
        GraphTensor::from_id(new_id, self.graph_ref)
    }

    pub fn expand<Dst: ConstShape, Ax: Axes>(self) -> GraphTensor<Dst>
    where
        S: BroadcastShapeTo<Dst, Ax>,
    {
        let graph = unsafe { &mut self.graph_ref.as_mut().unwrap().graph };
        let dim = Ax::as_array().into_iter().next().unwrap() as usize;
        let new_id = graph.add_node(Box::new(op::Expand(dim, Dst::realized_shape()[dim])));
        graph.add_edge(self.id, new_id, 0);
        GraphTensor::from_id(new_id, self.graph_ref)
    }

    pub fn reshape<N: ConstShape>(self) -> GraphTensor<N> {
        <S as AssertSameNumel<N>>::assert_same_numel();
        let graph = unsafe { &mut self.graph_ref.as_mut().unwrap().graph };
        let new_id = graph.add_node(Box::new(op::Reshape(N::realized_shape())));
        graph.add_edge(self.id, new_id, 0);
        GraphTensor::from_id(new_id, self.graph_ref)
    }

    pub fn sum_reduce<Dst: ConstShape, Ax: Axes>(self) -> GraphTensor<Dst>
    where
        S: HasAxes<Ax> + ReduceShapeTo<Dst, Ax>,
    {
        let graph = unsafe { &mut self.graph_ref.as_mut().unwrap().graph };
        let dim = Ax::as_array().into_iter().next().unwrap() as usize;
        let new_id = graph.add_node(Box::new(op::ReduceSum(dim)));
        graph.add_edge(self.id, new_id, 0);
        GraphTensor::from_id(new_id, self.graph_ref)
    }

    pub fn max_reduce<Dst: ConstShape, Ax: Axes>(self) -> GraphTensor<Dst>
    where
        S: HasAxes<Ax> + ReduceShapeTo<Dst, Ax>,
    {
        let graph = unsafe { &mut self.graph_ref.as_mut().unwrap().graph };
        let dim = Ax::as_array().into_iter().next().unwrap() as usize;
        let new_id = graph.add_node(Box::new(op::ReduceMax(dim)));
        graph.add_edge(self.id, new_id, 0);
        GraphTensor::from_id(new_id, self.graph_ref)
    }
}

// Matmul 2x2, 2x3 (broadcast 2 across batch), 2x4 (broadcast 2 across 2 batch dims), 3x3 (make sure shape matches up, multiply each consituent matrix)

// ABxBC -> AC
impl<const A: usize, const B: usize> GraphTensor<R2<A, B>> {
    pub fn matmul<const C: usize>(self, rhs: GraphTensor<R2<B, C>>) -> GraphTensor<R2<A, C>> {
        // Reshape
        let w: GraphTensor<R2<C, B>> = rhs.permute::<_, _, Axes2<1, 0>>();

        // Broadcasted Multiply
        let mul = self.expand::<R3<A, C, B>, _>() * w.expand::<R3<A, C, B>, _>();

        // Sum Reduce
        mul.sum_reduce::<_, Axis<2>>()
    }
}

impl<S: ConstShape> Add<GraphTensor<S>> for GraphTensor<S> {
    type Output = GraphTensor<S>;

    fn add(self, rhs: GraphTensor<S>) -> Self::Output {
        let graph = unsafe { &mut self.graph_ref.as_mut().unwrap().graph };
        let new_id = graph.add_node(Box::new(op::Add));
        graph.add_edge(self.id, new_id, 0);
        graph.add_edge(rhs.id, new_id, 1);
        GraphTensor::from_id(new_id, self.graph_ref)
    }
}

impl<S: ConstShape> Sub<GraphTensor<S>> for GraphTensor<S> {
    type Output = GraphTensor<S>;

    fn sub(self, rhs: GraphTensor<S>) -> Self::Output {
        let graph = unsafe { &mut self.graph_ref.as_mut().unwrap().graph };
        let new_id = graph.add_node(Box::new(op::Sub));
        graph.add_edge(self.id, new_id, 0);
        graph.add_edge(rhs.id, new_id, 1);
        GraphTensor::from_id(new_id, self.graph_ref)
    }
}

impl<S: ConstShape> Mul<GraphTensor<S>> for GraphTensor<S> {
    type Output = GraphTensor<S>;

    fn mul(self, rhs: GraphTensor<S>) -> Self::Output {
        let graph = unsafe { &mut self.graph_ref.as_mut().unwrap().graph };
        let new_id = graph.add_node(Box::new(op::Mul));
        graph.add_edge(self.id, new_id, 0);
        graph.add_edge(rhs.id, new_id, 1);
        GraphTensor::from_id(new_id, self.graph_ref)
    }
}

impl<S: ConstShape> Div<GraphTensor<S>> for GraphTensor<S> {
    type Output = GraphTensor<S>;

    fn div(self, rhs: GraphTensor<S>) -> Self::Output {
        let graph = unsafe { &mut self.graph_ref.as_mut().unwrap().graph };
        let new_id = graph.add_node(Box::new(op::Div));
        graph.add_edge(self.id, new_id, 0);
        graph.add_edge(rhs.id, new_id, 1);
        GraphTensor::from_id(new_id, self.graph_ref)
    }
}
