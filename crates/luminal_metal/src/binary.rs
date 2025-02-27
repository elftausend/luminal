use std::{any::Any, marker::PhantomData, mem::size_of, sync::Arc};

use itertools::Itertools;
use metal_rs::{
    objc::rc::autoreleasepool, Buffer, CommandBufferRef, CommandQueue, ComputePassDescriptor,
    ComputePipelineState, Device, MTLResourceOptions, MTLSize,
};
use rustc_hash::FxHashMap;

use crate::{
    compile_function, get_buffer_from_tensor, get_idx_valid_exps, input_dyn_dims,
    render_dyn_dim_inputs, select_const, DispatchNElements, MetalBuffer, MetalFloat, MetalKernel,
    MetalKernelWrapper, SetInt,
};

use super::prim::*;
use luminal::{
    op::{InputTensor, Operator},
    prelude::{
        petgraph::{stable_graph::NodeIndex, visit::EdgeRef, Direction},
        *,
    },
    shape::symbolic::BigExpression,
};

use super::other::MetalARange;

#[derive(LuminalEqTrue, LuminalPrint, Clone)]
pub struct MetalSub<T> {
    pipeline: ComputePipelineState,
    queue: CommandQueue,
    device: Device,
    dyn_symbols: Vec<char>,
    dyn_map: *const FxHashMap<char, usize>,
    _phantom: PhantomData<T>,
}

impl<T: MetalFloat> MetalSub<T> {
    pub fn new(
        a_shape: ShapeTracker,
        b_shape: ShapeTracker,
        device: Device,
        queue: CommandQueue,
        dyn_map: *const FxHashMap<char, usize>,
    ) -> Self {
        let (a_idx_exp, a_valid_exp) = get_idx_valid_exps(a_shape);
        let (b_idx_exp, b_valid_exp) = get_idx_valid_exps(b_shape);
        let (dyn_symbols, rendered) = render_dyn_dim_inputs(&[a_shape, b_shape], 4);
        let type_name = T::type_name();
        let code = format!(
            "
#include <metal_stdlib>
using namespace metal;
kernel void mkernel(device {type_name} *inp_a [[buffer(0)]], device {type_name} *inp_b [[buffer(1)]], device {type_name} *out [[buffer(2)]], device int& n_elements [[buffer(3)]], uint idx [[thread_position_in_grid]]{rendered}) {{
    if (idx < n_elements) {{
        out[idx] =
            (({a_valid_exp}) == 0 ? 0.0 : inp_a[{a_idx_exp}])
            - (({b_valid_exp}) == 0 ? 0.0 : inp_b[{b_idx_exp}]);
    }}
}}
");
        Self {
            pipeline: compile_function("mkernel", &code, &device),
            queue,
            device,
            dyn_symbols,
            dyn_map,
            _phantom: Default::default(),
        }
    }
}

impl<T> MetalKernel for MetalSub<T> {
    fn output_buffer_sizes(&self, input_shapes: &[ShapeTracker]) -> Vec<BigExpression> {
        vec![input_shapes[0].n_elements() * size_of::<T>()]
    }
    fn metal_forward(
        &self,
        inputs: &[(&Buffer, ShapeTracker)],
        command_buffer: &CommandBufferRef,
        _: &[&Buffer],
        output_buffers: &[&Buffer],
    ) {
        let inp_size = inputs[0].1.n_elements().to_usize().unwrap();
        let encoder =
            command_buffer.compute_command_encoder_with_descriptor(ComputePassDescriptor::new());
        encoder.set_compute_pipeline_state(&self.pipeline);

        // Set inputs
        encoder.set_buffer(0, Some(inputs[0].0), 0);
        encoder.set_buffer(1, Some(inputs[1].0), 0);
        encoder.set_buffer(2, Some(output_buffers[0]), 0);
        encoder.set_u32(3, inp_size as u32);
        input_dyn_dims(
            &self.dyn_symbols,
            unsafe { self.dyn_map.as_ref().unwrap() },
            encoder,
            4,
        );

        // Execute
        encoder.dispatch_1d(inp_size);
        encoder.end_encoding();
    }
}

impl<T: MetalFloat> Operator for MetalSub<T> {
    fn process(&mut self, tensors: Vec<(InputTensor, ShapeTracker)>) -> Vec<Tensor> {
        autoreleasepool(|| {
            let command_buffer = self.queue.new_command_buffer();
            let inp_size = tensors[0].1.n_elements().to_usize().unwrap();
            let out = self.device.new_buffer(
                (inp_size * std::mem::size_of::<T>()) as u64,
                MTLResourceOptions::StorageModeShared,
            );

            self.metal_forward(
                &[
                    (get_buffer_from_tensor(&tensors[0].0), tensors[0].1),
                    (get_buffer_from_tensor(&tensors[1].0), tensors[1].1),
                ],
                command_buffer,
                &[],
                &[&out],
            );

            command_buffer.commit();
            command_buffer.wait_until_completed();

            vec![Tensor::new(MetalBuffer(out))]
        })
    }

    fn custom(&mut self, key: &str, input: Box<dyn Any>) -> Option<Box<dyn Any>> {
        if key == "metal" {
            return Some(Box::new(MetalKernelWrapper(Arc::new(Box::new(
                self.clone(),
            )))));
        }
        // This op can accept non contiguous inputs
        if key == "non_contiguous" {
            return Some(Box::new(()));
        }
        if key == "elementwise" {
            return Some(Box::new("input0 - input1".to_string()));
        }
        if key == "recompile_shapes" {
            if let Some(input_shapes) = input.downcast_ref::<Vec<ShapeTracker>>() {
                *self = Self::new(
                    input_shapes[0],
                    input_shapes[1],
                    self.device.clone(),
                    self.queue.clone(),
                    self.dyn_map,
                )
            }
        }
        None
    }
}

#[derive(LuminalPrint, Default)]
pub struct MetalSubtractionCompiler<T: MetalFloat>(PhantomData<T>);

impl<T: MetalFloat> Compiler for MetalSubtractionCompiler<T> {
    fn compile<To: ToIdsMut>(&self, graph: &mut Graph, _: To) {
        let dev = Device::system_default().unwrap();
        let queue = dev.new_command_queue();
        let (mut neg_one, mut mul, mut add) = (
            NodeIndex::default(),
            NodeIndex::default(),
            NodeIndex::default(),
        );
        let mut searcher = select_const!(-1.0, T)
            .ptr(&mut neg_one)
            .edge(SelectOp::new().ty::<MetalMul<T>>().ptr(&mut mul))
            .edge(SelectOp::new().ty::<MetalAdd<T>>().ptr(&mut add))
            .search(graph);

        while searcher.next_match() {
            if check_no_delete(graph, &[neg_one, mul, add]) {
                continue;
            }
            let (a, a_edge) = graph
                .graph
                .edges_directed(add, petgraph::Direction::Incoming)
                .find(|e| e.source() != mul)
                .map(|e| (e.source(), e.weight().as_data().unwrap()))
                .unwrap();
            let (b, b_edge) = graph
                .graph
                .edges_directed(mul, petgraph::Direction::Incoming)
                .find(|e| e.source() != neg_one)
                .map(|e| (e.source(), e.weight().as_data().unwrap()))
                .unwrap();
            let b_final_shape = graph
                .graph
                .edges_connecting(mul, add)
                .next()
                .unwrap()
                .weight()
                .as_data()
                .unwrap()
                .2;
            if !b_final_shape.is_contiguous()
                || b_final_shape.is_sliced()
                || b_final_shape.is_padded()
            {
                continue;
            }
            let sub = graph
                .add_op(MetalSub::<T>::new(
                    a_edge.2,
                    b_edge.2,
                    dev.clone(),
                    queue.clone(),
                    &graph.dyn_map,
                ))
                .input(a, a_edge.1, a_edge.2)
                .input(b, b_edge.1, b_edge.2)
                .finish();
            move_outgoing_edge(add, sub, &mut graph.graph);

            if graph.get_dests(neg_one).len() == 1 {
                graph.graph.remove_node(neg_one);
            }
            graph.graph.remove_node(mul);
            graph.graph.remove_node(add);
        }
    }
}

#[derive(LuminalEqTrue, LuminalPrint, Clone)]
pub struct MetalEqual<T> {
    pipeline: ComputePipelineState,
    queue: CommandQueue,
    device: Device,
    dyn_symbols: Vec<char>,
    dyn_map: *const FxHashMap<char, usize>,
    _phantom: PhantomData<T>,
}

impl<T: MetalFloat> MetalEqual<T> {
    pub fn new(
        a_shape: ShapeTracker,
        b_shape: ShapeTracker,
        device: Device,
        queue: CommandQueue,
        dyn_map: *const FxHashMap<char, usize>,
    ) -> Self {
        let (a_idx_exp, a_valid_exp) = get_idx_valid_exps(a_shape);
        let (b_idx_exp, b_valid_exp) = get_idx_valid_exps(b_shape);
        let (dyn_symbols, rendered) = render_dyn_dim_inputs(&[a_shape, b_shape], 4);
        let type_name = T::type_name();
        let code = format!(
            "
#include <metal_stdlib>
using namespace metal;
kernel void mkernel(device {type_name} *inp_a [[buffer(0)]], device {type_name} *inp_b [[buffer(1)]], device {type_name} *out [[buffer(2)]], device int& n_elements [[buffer(3)]], uint idx [[thread_position_in_grid]]{rendered}) {{
    if (idx < n_elements) {{
        {type_name} a_val = (({a_valid_exp}) == 0 ? 0.0 : inp_a[{a_idx_exp}]);
        {type_name} b_val = (({b_valid_exp}) == 0 ? 0.0 : inp_b[{b_idx_exp}]);
        out[idx] = ({type_name})(a_val == b_val);
    }}
}}
");
        Self {
            pipeline: compile_function("mkernel", &code, &device),
            queue,
            device,
            dyn_symbols,
            dyn_map,
            _phantom: Default::default(),
        }
    }
}

impl<T> MetalKernel for MetalEqual<T> {
    fn output_buffer_sizes(&self, input_shapes: &[ShapeTracker]) -> Vec<BigExpression> {
        vec![input_shapes[0].n_elements() * size_of::<T>()]
    }
    fn metal_forward(
        &self,
        inputs: &[(&Buffer, ShapeTracker)],
        command_buffer: &CommandBufferRef,
        _: &[&Buffer],
        output_buffers: &[&Buffer],
    ) {
        let inp_size = inputs[0].1.n_elements().to_usize().unwrap();

        let encoder =
            command_buffer.compute_command_encoder_with_descriptor(ComputePassDescriptor::new());
        encoder.set_compute_pipeline_state(&self.pipeline);

        // Set inputs
        encoder.set_buffer(0, Some(inputs[0].0), 0);
        encoder.set_buffer(1, Some(inputs[1].0), 0);
        encoder.set_buffer(2, Some(output_buffers[0]), 0);
        encoder.set_u32(3, inp_size as u32);
        input_dyn_dims(
            &self.dyn_symbols,
            unsafe { self.dyn_map.as_ref().unwrap() },
            encoder,
            4,
        );

        // Execute
        encoder.dispatch_1d(inp_size);
        encoder.end_encoding();
    }
}

impl<T: MetalFloat> Operator for MetalEqual<T> {
    fn process(&mut self, tensors: Vec<(InputTensor, ShapeTracker)>) -> Vec<Tensor> {
        autoreleasepool(|| {
            let command_buffer = self.queue.new_command_buffer();
            let inp_size = tensors[0].1.n_elements().to_usize().unwrap();
            let out = self.device.new_buffer(
                (inp_size * std::mem::size_of::<T>()) as u64,
                MTLResourceOptions::StorageModeShared,
            );

            self.metal_forward(
                &[
                    (get_buffer_from_tensor(&tensors[0].0), tensors[0].1),
                    (get_buffer_from_tensor(&tensors[1].0), tensors[1].1),
                ],
                command_buffer,
                &[],
                &[&out],
            );

            command_buffer.commit();
            command_buffer.wait_until_completed();

            vec![Tensor::new(MetalBuffer(out))]
        })
    }

    fn custom(&mut self, key: &str, input: Box<dyn Any>) -> Option<Box<dyn Any>> {
        if key == "metal" {
            return Some(Box::new(MetalKernelWrapper(Arc::new(Box::new(
                self.clone(),
            )))));
        }
        // This op can accept non contiguous inputs
        if key == "non_contiguous" {
            return Some(Box::new(()));
        }
        if key == "elementwise" {
            return Some(Box::new("input0 == input1 ? 1.0 : 0.0".to_string()));
        }
        if key == "recompile_shapes" {
            if let Some(input_shapes) = input.downcast_ref::<Vec<ShapeTracker>>() {
                *self = Self::new(
                    input_shapes[0],
                    input_shapes[1],
                    self.device.clone(),
                    self.queue.clone(),
                    self.dyn_map,
                )
            }
        }
        None
    }
}

#[derive(LuminalPrint, Default)]
pub struct MetalEqualCompiler<T: MetalFloat>(PhantomData<T>);

impl<T: MetalFloat> Compiler for MetalEqualCompiler<T> {
    fn compile<To: ToIdsMut>(&self, graph: &mut Graph, _: To) {
        let dev = Device::system_default().unwrap();
        let queue = dev.new_command_queue();
        let (mut less_than1, mut less_than2, mut add, mut one, mut sub) = (
            NodeIndex::default(),
            NodeIndex::default(),
            NodeIndex::default(),
            NodeIndex::default(),
            NodeIndex::default(),
        );
        let s = select_const!(1.0, T).ptr(&mut one).edge(
            SelectOp::new()
                .ty::<MetalLessThan<T>>()
                .ptr(&mut less_than1)
                .edge(
                    SelectOp::new()
                        .ty::<MetalLessThan<T>>()
                        .ptr(&mut less_than2)
                        .edge(SelectOp::new().ty::<MetalAdd<T>>().ptr(&mut add)),
                )
                .edge(SelectOp::new().ty::<MetalSub<T>>().ptr(&mut sub)),
        );

        let mut searcher = s.search(graph);
        while searcher.next_match() {
            let lt1_inputs = graph
                .graph
                .neighbors_directed(less_than1, Direction::Incoming)
                .sorted()
                .collect::<Vec<_>>();
            let lt2_inputs = graph
                .graph
                .neighbors_directed(less_than2, Direction::Incoming)
                .sorted()
                .collect::<Vec<_>>();
            if lt1_inputs != lt2_inputs {
                continue;
            }
            let inputs = graph
                .graph
                .edges_directed(less_than1, Direction::Incoming)
                .sorted_by_key(|e| e.weight().as_data().unwrap().0)
                .map(|e| e.source())
                .collect::<Vec<_>>();
            let (a, b) = (inputs[0], inputs[1]);
            if check_no_delete(graph, &[less_than1, less_than2, add, one, sub]) {
                continue;
            }
            let a_edge = graph
                .graph
                .edge_weight(
                    graph
                        .graph
                        .edges_connecting(a, less_than1)
                        .next()
                        .unwrap()
                        .id(),
                )
                .unwrap()
                .as_data()
                .unwrap();
            let b_edge = graph
                .graph
                .edge_weight(
                    graph
                        .graph
                        .edges_connecting(b, less_than1)
                        .next()
                        .unwrap()
                        .id(),
                )
                .unwrap()
                .as_data()
                .unwrap();
            let equals = graph
                .add_op(MetalEqual::<T>::new(
                    a_edge.2,
                    b_edge.2,
                    dev.clone(),
                    queue.clone(),
                    &graph.dyn_map,
                ))
                .input(a, a_edge.1, a_edge.2)
                .input(b, b_edge.1, b_edge.2)
                .finish();
            move_outgoing_edge(sub, equals, &mut graph.graph);

            graph.graph.remove_node(sub);
            graph.safe_remove_node(add, 0);
            graph.safe_remove_node(one, 0);
            graph.safe_remove_node(less_than2, 0);
            graph.safe_remove_node(less_than1, 0);
            searcher.clear_cached_results();
        }
    }
}

#[derive(LuminalEqFalse, LuminalPrint, Clone)]
pub struct MetalGather<T> {
    pipeline: ComputePipelineState,
    device: Device,
    queue: CommandQueue,
    pub embed_dim: usize,
    _phantom: PhantomData<T>,
}

impl<T: MetalFloat> MetalGather<T> {
    fn new(device: Device, queue: CommandQueue, embed_dim: usize) -> Self {
        let type_name = T::type_name();
        Self {pipeline: compile_function("metal_gather", &format!(
            "
#include <metal_stdlib>
using namespace metal;
kernel void metal_gather(device float *inp [[buffer(0)]], device {type_name} *weights [[buffer(1)]], device {type_name} *out [[buffer(2)]], device int& n_embeddings [[buffer(3)]], device int& embedding_dim [[buffer(4)]], uint2 i_ [[thread_position_in_grid]]) {{
    if (i_.x < n_embeddings && i_.y < embedding_dim) {{
        out[i_.x * embedding_dim + i_.y] = weights[(int)inp[i_.x] * embedding_dim + i_.y];
    }}
}}"), &device), device, embed_dim, queue, _phantom: Default::default()}
    }
}

impl<T: MetalFloat> Operator for MetalGather<T> {
    fn process(&mut self, tensors: Vec<(InputTensor, ShapeTracker)>) -> Vec<Tensor> {
        autoreleasepool(|| {
            // Setup buffers
            let indexes = tensors[0]
                .0
                .borrowed()
                .data
                .as_any()
                .downcast_ref::<Vec<f32>>()
                .unwrap();
            let index_buffer = self.device.new_buffer_with_data(
                unsafe { std::mem::transmute(indexes.as_ptr()) },
                (indexes.len() * std::mem::size_of::<f32>()) as u64,
                MTLResourceOptions::StorageModeShared,
            );
            let b_inp = tensors[1]
                .0
                .borrowed()
                .data
                .as_any()
                .downcast_ref::<MetalBuffer>()
                .unwrap();

            // Setup command queue / command buffer / encoder
            let command_buffer = self.queue.new_command_buffer();

            let out = self.device.new_buffer(
                (indexes.len() * self.embed_dim * std::mem::size_of::<T>()) as u64,
                MTLResourceOptions::StorageModeShared,
            );

            let encoder = command_buffer
                .compute_command_encoder_with_descriptor(ComputePassDescriptor::new());
            encoder.set_compute_pipeline_state(&self.pipeline);

            // Set inputs
            encoder.set_buffer(0, Some(&index_buffer), 0);
            encoder.set_buffer(1, Some(b_inp), 0);
            encoder.set_buffer(2, Some(&out), 0);
            encoder.set_u32(3, indexes.len() as u32);
            encoder.set_u32(4, self.embed_dim as u32);

            // Execute
            encoder.dispatch_threads(
                MTLSize {
                    width: indexes.len() as u64,
                    height: self.embed_dim as u64,
                    depth: 1,
                },
                MTLSize {
                    width: 16,
                    height: 16,
                    depth: 1,
                },
            );
            encoder.end_encoding();

            command_buffer.commit();
            command_buffer.wait_until_completed();

            vec![Tensor::new(MetalBuffer(out))]
        })
    }
}

#[derive(LuminalPrint, Default)]
pub struct MetalGatherCompiler<T: MetalFloat>(PhantomData<T>);

impl<T: MetalFloat> Compiler for MetalGatherCompiler<T> {
    fn compile<To: ToIdsMut>(&self, graph: &mut Graph, _: To) {
        let dev = Device::system_default().unwrap();
        let queue = dev.new_command_queue();
        let (mut ind_copy, mut arange, mut equal, mut mul, mut sum_reduce) = (
            NodeIndex::default(),
            NodeIndex::default(),
            NodeIndex::default(),
            NodeIndex::default(),
            NodeIndex::default(),
        );
        let s = SelectOp::new()
            .ty::<MetalARange<T>>()
            .ptr(&mut arange)
            .edge(
                SelectOp::new()
                    .ty::<MetalCopyToDevice<T>>()
                    .ptr(&mut ind_copy)
                    .edge(SelectOp::new().ty::<MetalEqual<T>>().ptr(&mut equal)),
            )
            .edge(SelectOp::new().ty::<MetalMul<T>>().ptr(&mut mul))
            .edge(
                SelectOp::new()
                    .ty::<MetalSumReduce<T>>()
                    .ptr(&mut sum_reduce),
            );
        let mut searcher = s.search(graph);
        while searcher.next_match() {
            if check_no_delete(graph, &[arange, equal, mul, sum_reduce]) {
                continue;
            }
            let embedding_dim = graph
                .graph
                .edges_directed(mul, Direction::Incoming)
                .find(|e| e.source() != equal && !e.weight().is_schedule())
                .unwrap()
                .weight()
                .as_data()
                .unwrap()
                .2
                .shape()[2]
                .to_usize()
                .unwrap();
            let gather = graph
                .add_op(MetalGather::<T>::new(
                    dev.clone(),
                    queue.clone(),
                    embedding_dim,
                ))
                .finish();
            move_incoming_edge(ind_copy, gather, &mut graph.graph);
            graph.safe_remove_node(equal, 1);
            move_incoming_edge(mul, gather, &mut graph.graph);
            move_outgoing_edge(sum_reduce, gather, &mut graph.graph);
            graph.graph.remove_node(sum_reduce);
            graph.safe_remove_node(mul, 0);
            graph.safe_remove_node(ind_copy, 0);
            graph.safe_remove_node(arange, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use luminal::{prelude::*, tests::assert_close};

    use crate::MetalCompiler;
    #[test]
    fn test_subtraction() {
        let mut cx = Graph::new();
        let a = cx
            .tensor::<R1<10>>()
            .set(vec![1., 2., 3., 4., 5., 6., 7., 8., 9., 10.]);
        let b = cx.tensor::<R0>().set(vec![1.]);
        let mut c = (a - b.expand()).retrieve();
        let mut d = (-a + b.expand()).retrieve();

        cx.execute();

        let unopt_c = c.data();
        c.drop();
        let unopt_d = d.data();
        d.drop();

        cx.compile(MetalCompiler::<f16>::default(), (&mut c, &mut d));
        cx.execute();

        assert_close(&unopt_c, &c.data());
        assert_close(&unopt_d, &d.data());
    }
}
