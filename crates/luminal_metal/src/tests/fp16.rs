use dfdx::prelude::{Module as DfdxModule, *};
use itertools::Itertools;
use metal_rs::objc::rc::autoreleasepool;
use num_traits::Float;
use rand::{rngs::StdRng, SeedableRng};

use luminal::{
    nn::{activation::ReLU, linear::Linear, norm::RMSNorm},
    prelude::{symbolic::Expression, Module, *},
    tests::{assert_close, assert_close_precision, assert_exact, random_vec, random_vec_rng},
};

use crate::MetalCompiler;

#[test]
fn test_contiguous() {
    let mut cx = Graph::new();
    let data = random_vec(12);
    let a = cx.tensor::<R2<3, 4>>().set(data.clone());
    let mut b = a.permute::<R2<4, 3>, _>().reshape::<R2<12, 1>>().retrieve();
    cx.compile(MetalCompiler::<f16>::default(), &mut b);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev
        .tensor_from_vec(data, (dfdx::shapes::Const::<3>, dfdx::shapes::Const::<4>))
        .to_dtype::<f16>();
    let d_b = d_a.permute::<Rank2<4, 3>, _>().reshape::<Rank2<12, 1>>();

    assert_close(&b.data(), &d_b.to_dtype::<f32>().as_vec());
}

#[test]
fn test_softmax() {
    let mut cx = Graph::new();
    let data = random_vec(12);
    let a = cx.tensor::<R2<1, 12>>().set(data.clone());
    let mut b = a.softmax::<1>().retrieve();
    cx.compile(MetalCompiler::<f16>::default(), &mut b);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev
        .tensor_from_vec(data, (dfdx::shapes::Const::<1>, dfdx::shapes::Const::<12>))
        .to_dtype::<f16>();
    let d_b = d_a.softmax::<dfdx::shapes::Axis<1>>();

    assert_close(&b.data(), &d_b.to_dtype::<f32>().as_vec());
}

#[test]
fn test_rotate() {
    let mut cx = Graph::new();
    const D: usize = 2;
    const S: usize = 2;
    const H: usize = 2;
    let data = random_vec(D * S * H);
    let a = cx
        .tensor::<R4<1, D, S, H>>()
        .set(data)
        .keep()
        .permute::<_, luminal::prelude::Axes4<0, 2, 1, 3>>();
    let x1 = a.slice((.., .., .., ..Expression::from(H / 2)));
    let x2 = a.slice((.., .., .., Expression::from(H / 2)..));
    let mut rotated_a = (-x2)
        .concat_along::<R4<1, S, D, H>, luminal::prelude::Axis<3>, _>(x1)
        .retrieve();
    cx.execute();
    let unopt = rotated_a.data();

    cx.compile(MetalCompiler::<f16>::default(), &mut rotated_a);
    cx.execute();

    assert_close(&unopt, &rotated_a.data());
}

#[test]
fn test_constant() {
    let mut cx = Graph::new();
    let a = cx.constant_expr('a');
    let mut a = (a * a).retrieve();
    cx.compile(MetalCompiler::<f16>::default(), &mut a);

    cx.set_dyn_dim('a', 10);
    cx.execute();
    assert_exact(&a.data(), &[100.0]);
    a.drop();
    cx.set_dyn_dim('a', 25);
    cx.execute();
    assert_exact(&a.data(), &[625.0]);
}

#[test]
fn test_log2() {
    let mut cx = Graph::new();
    let data = random_vec(3);
    let a = cx.tensor::<R1<3>>().set(data.clone());
    let mut b = a.log2().retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut b);
    cx.execute();

    assert_close(
        &b.data(),
        &data
            .into_iter()
            .map(|i| f16::from_f32(i).log2().to_f32())
            .collect::<Vec<_>>(),
    );
}

#[test]
fn test_exp2() {
    let mut cx = Graph::new();
    let data = random_vec(3);
    let a = cx.tensor::<R1<3>>().set(data.clone());
    let mut b = a.exp2().retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut b);
    cx.execute();

    assert_close(
        &b.data(),
        &data.into_iter().map(|i: f32| i.exp2()).collect::<Vec<_>>(),
    );
}

#[test]
fn test_recip() {
    let mut cx = Graph::new();
    let a = cx.tensor::<R1<3>>().set(vec![1., 2., 4096.]);
    let mut b = a.recip().retrieve();
    cx.compile(MetalCompiler::<f16>::default(), &mut b);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev.tensor([1., 2., 4096.]).to_dtype::<f16>();
    let d_b = d_a.recip();

    assert_close(&b.data(), &d_b.to_dtype::<f32>().as_vec());
}

#[test]
fn test_sin() {
    let mut cx = Graph::new();
    let a = cx.tensor::<R1<3>>().set(vec![1., 2., 3.]);
    let mut b = a.sin().retrieve();
    cx.compile(MetalCompiler::<f16>::default(), &mut b);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev.tensor([1., 2., 3.]).to_dtype::<f16>();
    let d_b = d_a.sin();

    assert_close(&b.data(), &d_b.to_dtype::<f32>().as_vec());
}

#[test]
fn test_sqrt() {
    let mut cx = Graph::new();
    let a = cx.tensor::<R1<3>>().set(vec![1., 2., 3.]);
    let mut b = a / a.sqrt();
    b.retrieve();
    cx.compile(MetalCompiler::<f16>::default(), &mut b);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev.tensor([1., 2., 3.]).to_dtype::<f16>();
    let d_b = d_a.clone() / d_a.sqrt();

    assert_close(&b.data(), &d_b.to_dtype::<f32>().as_vec());
}

#[test]
fn test_add() {
    let mut cx = Graph::new();
    let a = cx.tensor::<R1<3>>().set(vec![1., 2., 3.]);
    let b = cx.tensor::<R1<3>>().set(vec![1., 2., 3.]);
    let mut c = (a + b).retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut c);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev.tensor([1., 2., 3.]).to_dtype::<f16>();
    let d_b = d_dev.tensor([1., 2., 3.]).to_dtype::<f16>();
    let d_c = d_a + d_b;

    assert_close(&c.data(), &d_c.to_dtype::<f32>().as_vec());
}

#[test]
fn test_sub() {
    let mut cx = Graph::new();
    let a = cx.tensor::<R1<3>>().set(vec![1., 2., 3.]);
    let b = cx.tensor::<R1<3>>().set(vec![1., 2., 3.]);
    let mut c = a - b;
    c.retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut c);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev.tensor([1., 2., 3.]).to_dtype::<f16>();
    let d_b = d_dev.tensor([1., 2., 3.]).to_dtype::<f16>();
    let d_c = d_a - d_b;

    assert_close(&c.data(), &d_c.to_dtype::<f32>().as_vec());
}

#[test]
fn test_square() {
    let mut cx = Graph::new();
    let mut rng = rand::thread_rng();
    let data = random_vec_rng(40960, &mut rng);
    let a = cx
        .tensor::<(Dyn<'b'>, Dyn<'s'>, luminal::prelude::Const<4096>)>()
        .set_dyn(data.clone(), &[1, 10, 4096]);
    let mut b = a * a;
    b.retrieve();

    cx.compile(<(GenericCompiler, MetalCompiler<f16>)>::default(), &mut b);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev
        .tensor_from_vec::<Rank3<1, 10, 4096>>(
            data,
            (
                dfdx::prelude::Const::<1>,
                dfdx::prelude::Const::<10>,
                dfdx::prelude::Const::<4096>,
            ),
        )
        .to_dtype::<f16>();
    let d_b = d_a.clone() * d_a;

    assert_close(&b.data(), &d_b.to_dtype::<f32>().as_vec());
}

#[test]
fn test_mul() {
    let mut cx = Graph::new();
    let a = cx.tensor::<R1<3>>().set(vec![1., 2., 3.]);
    let b = cx.tensor::<R1<3>>().set(vec![1., 2., 3.]);
    let mut c = a * b;
    c.retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut c);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev.tensor([1., 2., 3.]).to_dtype::<f16>();
    let d_b = d_dev.tensor([1., 2., 3.]).to_dtype::<f16>();
    let d_c = d_a * d_b;

    assert_close(&c.data(), &d_c.to_dtype::<f32>().as_vec());
}

#[test]
fn test_mul2() {
    let mut cx = Graph::new();
    let a = cx
        .tensor::<(
            luminal::prelude::Const<1>,
            luminal::prelude::Const<1>,
            Dyn<'a'>,
            Dyn<'a'>,
        )>()
        .set_dyn(vec![82.4, 783.0, 99.6, 974.5], &[1, 1, 2, 2]);
    let b = cx.tensor::<R0>().set(vec![0.57735026]);
    let mut c = (a * b.expand()).retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut c);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev
        .tensor([[[[82.4, 783.0], [99.6, 974.5]]]])
        .to_dtype::<f16>();
    let d_b = d_dev.tensor(0.57735026).to_dtype::<f16>();
    let d_c = d_a * d_b.broadcast::<_, dfdx::shapes::Axes4<0, 1, 2, 3>>();

    assert_exact(&c.data(), &d_c.to_dtype::<f32>().as_vec());
}

#[test]
fn test_div() {
    let mut cx = Graph::new();
    let a = cx.tensor::<R1<3>>().set(vec![1., 2., 3.]);
    let b = cx.tensor::<R1<3>>().set(vec![1., 2., 3.]);
    let mut c = a / b;
    c.retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut c);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev.tensor([1., 2., 3.]).to_dtype::<f16>();
    let d_b = d_dev.tensor([1., 2., 3.]).to_dtype::<f16>();
    let d_c = d_a / d_b;

    assert_close(&c.data(), &d_c.to_dtype::<f32>().as_vec());
}

#[test]
fn test_max() {
    let mut cx = Graph::new();
    let a = cx.tensor::<R1<3>>().set(vec![1., 2., 3.]);
    let b = cx.tensor::<R1<3>>().set(vec![1., 2., 3.]);
    let mut c = a.max(b).retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut c);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev.tensor([1., 2., 3.]).to_dtype::<f16>();
    let d_b = d_dev.tensor([1., 2., 3.]).to_dtype::<f16>();
    let d_c = d_a.maximum(d_b);

    assert_close(&c.data(), &d_c.to_dtype::<f32>().as_vec());
}

#[test]
fn test_mod() {
    let mut cx = Graph::new();
    let a_data = random_vec(3);
    let b_data = random_vec(3);
    let a = cx.tensor::<R1<3>>().set(a_data.clone());
    let b = cx.tensor::<R1<3>>().set(b_data.clone());
    let mut c = a % b;
    c.retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut c);
    cx.execute();

    // No dfdx equivalent

    assert_close(
        &c.data(),
        &a_data
            .into_iter()
            .zip(b_data)
            .map(|(a, b)| a % b)
            .collect_vec(),
    );
}

// Reduction op tests

#[test]
fn test_sum_reduce() {
    let data = random_vec(40960);
    let mut cx = Graph::new();
    let a = cx.tensor::<R3<1, 10, 4096>>().set(data.clone());
    let mut b = a.sum_reduce::<_, luminal::prelude::Axis<2>>().retrieve();
    let mut c = a.sum_reduce::<_, luminal::prelude::Axis<1>>().retrieve();
    let mut d = a.sum_reduce::<_, luminal::prelude::Axis<0>>().retrieve();

    cx.compile(MetalCompiler::<f16>::default(), (&mut b, &mut c, &mut d));
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev
        .tensor_from_vec(
            data,
            (
                dfdx::shapes::Const::<1>,
                dfdx::shapes::Const::<10>,
                dfdx::shapes::Const::<4096>,
            ),
        )
        .to_dtype::<f16>();
    let d_b = d_a.clone().sum::<_, dfdx::shapes::Axis<2>>();
    let d_c = d_a.clone().sum::<_, dfdx::shapes::Axis<1>>();
    let d_d = d_a.sum::<_, dfdx::shapes::Axis<0>>();
    assert_close(&b.data(), &d_b.to_dtype::<f32>().as_vec());
    assert_close(&c.data(), &d_c.to_dtype::<f32>().as_vec());
    assert_close(&d.data(), &d_d.to_dtype::<f32>().as_vec());
}

#[test]
fn test_sum_reduce2() {
    let mut cx = Graph::new();
    let data = random_vec(32 * 10 * 10 * 128);
    let a = cx.tensor::<R5<1, 32, 10, 10, 128>>().set(data.clone());
    let mut d = a.sum_reduce::<_, luminal::prelude::Axis<2>>().retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut d);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev
        .tensor_from_vec(
            data,
            (
                dfdx::shapes::Const::<1>,
                dfdx::shapes::Const::<32>,
                dfdx::shapes::Const::<10>,
                dfdx::shapes::Const::<10>,
                dfdx::shapes::Const::<128>,
            ),
        )
        .to_dtype::<f16>();
    let d_d = d_a.sum::<_, dfdx::shapes::Axis<2>>();

    assert_exact(&d.data(), &d_d.to_dtype::<f32>().as_vec());
}

#[test]
fn test_max_reduce() {
    let data = random_vec(40960);
    let mut cx = Graph::new();
    let a = cx.tensor::<R3<1, 10, 4096>>().set(data.clone());
    let mut b = a.max_reduce::<_, luminal::prelude::Axis<2>>().retrieve();
    let mut c = a.max_reduce::<_, luminal::prelude::Axis<1>>().retrieve();
    let mut d = a.max_reduce::<_, luminal::prelude::Axis<0>>().retrieve();

    cx.compile(MetalCompiler::<f16>::default(), (&mut b, &mut c, &mut d));
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev
        .tensor_from_vec(
            data,
            (
                dfdx::shapes::Const::<1>,
                dfdx::shapes::Const::<10>,
                dfdx::shapes::Const::<4096>,
            ),
        )
        .to_dtype::<f16>();
    let d_b = d_a.clone().max::<_, dfdx::shapes::Axis<2>>();
    let d_c = d_a.clone().max::<_, dfdx::shapes::Axis<1>>();
    let d_d = d_a.max::<_, dfdx::shapes::Axis<0>>();
    assert_close(&b.data(), &d_b.to_dtype::<f32>().as_vec());
    assert_close(&c.data(), &d_c.to_dtype::<f32>().as_vec());
    assert_close(&d.data(), &d_d.to_dtype::<f32>().as_vec());
}

#[test]
fn test_mean_reduce() {
    let data = random_vec(40960);
    let mut cx = Graph::new();
    let a = cx.tensor::<R3<1, 10, 4096>>().set(data.clone());
    let mut b = a.mean_reduce::<_, luminal::prelude::Axis<2>>().retrieve();
    let mut c = a.mean_reduce::<_, luminal::prelude::Axis<1>>().retrieve();
    let mut d = a.mean_reduce::<_, luminal::prelude::Axis<0>>().retrieve();

    cx.compile(MetalCompiler::<f16>::default(), (&mut b, &mut c, &mut d));
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev
        .tensor_from_vec(
            data,
            (
                dfdx::shapes::Const::<1>,
                dfdx::shapes::Const::<10>,
                dfdx::shapes::Const::<4096>,
            ),
        )
        .to_dtype::<f16>();
    let d_b = d_a.clone().mean::<_, dfdx::shapes::Axis<2>>();
    let d_c = d_a.clone().mean::<_, dfdx::shapes::Axis<1>>();
    let d_d = d_a.mean::<_, dfdx::shapes::Axis<0>>();
    assert_close(&b.data(), &d_b.to_dtype::<f32>().as_vec());
    assert_close(&c.data(), &d_c.to_dtype::<f32>().as_vec());
    assert_close(&d.data(), &d_d.to_dtype::<f32>().as_vec());
}

// #[test]
// fn test_matmul_simple() {
//     let mut cx = Graph::new();
//     let a_data = random_vec(256 * 256);
//     let b_data = random_vec(256 * 256);
//     let a = cx.tensor::<R2<256, 256>>().set(a_data.clone());
//     let b = cx.tensor::<R2<256, 256>>().set(b_data.clone());
//     let mut c = a.matmul(b).retrieve();

//     cx.compile(MetalCompiler::<f16>::default(), &mut c);
//     cx.execute();

//     let d_dev = Cpu::default();
//     let d_a = d_dev.tensor_from_vec(
//         a_data,
//         (dfdx::shapes::Const::<256>, dfdx::shapes::Const::<256>),
//     );
//     let d_b = d_dev.tensor_from_vec(
//         b_data,
//         (dfdx::shapes::Const::<256>, dfdx::shapes::Const::<256>),
//     );
//     let d_c = d_a.to_dtype::<f16>().matmul(d_b.to_dtype::<f16>());

//     assert_close(&c.data(), &d_c.to_dtype::<f32>().as_vec());
// }

#[test]
fn test_matmul() {
    let d_dev = Cpu::default();
    let mut cx = Graph::new();
    let mut rng = StdRng::seed_from_u64(0);
    let a = cx.tensor::<(Dyn<'M'>, Dyn<'K'>)>();
    let b = cx.tensor::<(Dyn<'K'>, Dyn<'N'>)>();
    let mut c = a.matmul(b).retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut c);
    for m in (1..23).step_by(4) {
        for k in (1..35).step_by(3) {
            for n in (1..70).step_by(7) {
                autoreleasepool(|| {
                    let a_data = random_vec_rng(m * k, &mut rng);
                    let b_data = random_vec_rng(k * n, &mut rng);
                    a.set_dyn(a_data.clone(), &[m, k]);
                    b.set_dyn(b_data.clone(), &[k, n]);

                    cx.execute();

                    let d_a = d_dev.tensor_from_vec(a_data, (m, k));
                    let d_b = d_dev.tensor_from_vec(b_data, (k, n));
                    let d_c = d_a.matmul(d_b);
                    assert_close_precision(&c.data(), &d_c.to_dtype::<f32>().as_vec(), 2);
                    c.drop();
                })
            }
        }
    }
}

#[test]
fn test_attn_matmul() {
    let mut cx = Graph::new();
    let mut rng = StdRng::seed_from_u64(0);
    let a_data = random_vec_rng(32 * 11 * 128, &mut rng);
    let b_data = random_vec_rng(32 * 11 * 128, &mut rng);
    let a = cx
        .named_tensor::<R4<1, 32, 11, 128>>("Input")
        .set(a_data.clone())
        .keep();
    let b = cx
        .named_tensor::<R4<1, 32, 128, 11>>("Input")
        .set(b_data.clone())
        .keep();
    let mut c = a.matmul(b).retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut c);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev
        .tensor_from_vec(
            a_data,
            (
                dfdx::shapes::Const::<1>,
                dfdx::shapes::Const::<32>,
                dfdx::shapes::Const::<11>,
                dfdx::shapes::Const::<128>,
            ),
        )
        .to_dtype::<f16>();
    let d_b = d_dev
        .tensor_from_vec(
            b_data,
            (
                dfdx::shapes::Const::<1>,
                dfdx::shapes::Const::<32>,
                dfdx::shapes::Const::<128>,
                dfdx::shapes::Const::<11>,
            ),
        )
        .to_dtype::<f16>();
    let d_c = d_a.matmul(d_b);
    assert_exact(&c.data(), &d_c.to_dtype::<f32>().as_vec());
}

#[test]
fn test_batch_matmul() {
    let m = 12;
    let mut cx = Graph::new();
    let mut rng = StdRng::seed_from_u64(0);
    let a = cx.tensor::<(Dyn<'B'>, Dyn<'M'>, Dyn<'K'>)>();
    let b = cx.tensor::<(Dyn<'K'>, Dyn<'N'>)>();
    let mut c = a.matmul(b).retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut c);
    for batch in (2..23).step_by(4) {
        for k in (1..35).step_by(3) {
            for n in (1..48).step_by(7) {
                println!("B: {batch} M: {m} K: {k} N: {n}");
                autoreleasepool(|| {
                    let a_data = random_vec_rng(batch * m * k, &mut rng);
                    let b_data = random_vec_rng(k * n, &mut rng);
                    a.set_dyn(a_data.clone(), &[batch, m, k]);
                    b.set_dyn(b_data.clone(), &[k, n]);
                    cx.execute();

                    let d_dev = Cpu::default();
                    let d_a = d_dev.tensor_from_vec(a_data, (batch, m, k));
                    let d_b = d_dev.tensor_from_vec(b_data, (k, n));
                    let d_c = d_a.matmul(d_b);

                    assert_close_precision(&c.data(), &d_c.to_dtype::<f32>().as_vec(), 2);
                    c.drop();
                });
            }
        }
    }
}

// #[test]
// fn test_batch_matmul_transpose() {
//     const B: usize = 1;
//     const M: usize = 48; // Any
//     const K: usize = 4096; // >= 16, multiple of 16
//     const N: usize = 4096; // >= 256, multiple of 256
//     let mut cx = Graph::new();
//     let mut rng = StdRng::seed_from_u64(0);

//     let a_data = random_vec_rng(B * M * K, &mut rng);
//     let a = cx.named_tensor::<R3<B, M, K>>("A").set(a_data.clone());
//     let b_data = random_vec_rng(K * N, &mut rng);
//     let b = cx.named_tensor::<R2<N, K>>("B").set(b_data.clone());
//     let a_t_data = random_vec_rng(B * K * M, &mut rng);
//     let a_t = cx.named_tensor::<R3<B, K, M>>("A_T").set(a_t_data.clone());
//     let b_t_data = random_vec_rng(K * N, &mut rng);
//     let b_t = cx.named_tensor::<R2<K, N>>("B_T").set(b_t_data.clone());

//     let mut a_b = a
//         .matmul(b.permute::<_, luminal::prelude::Axes2<1, 0>>())
//         .retrieve();
//     let mut a_b_t = a.matmul(b_t).retrieve();
//     let mut a_t_b = a_t
//         .permute::<_, luminal::prelude::Axes3<0, 2, 1>>()
//         .matmul(b.permute::<_, luminal::prelude::Axes2<1, 0>>())
//         .retrieve();
//     let mut a_t_b_t = a_t
//         .permute::<_, luminal::prelude::Axes3<0, 2, 1>>()
//         .matmul(b_t)
//         .retrieve();

//     cx.compile(
//         <(GenericCompiler, MetalCompiler<f16>)>::default(),
//         (&mut a_b, &mut a_b_t, &mut a_t_b, &mut a_t_b_t),
//     );
//     cx.execute();

//     let d_dev = Cpu::default();
//     let d_a = d_dev.tensor_from_vec(
//         a_data,
//         (
//             dfdx::shapes::Const::<B>,
//             dfdx::shapes::Const::<M>,
//             dfdx::shapes::Const::<K>,
//         ),
//     );
//     let d_b = d_dev.tensor_from_vec(b_data, (dfdx::shapes::Const::<N>, dfdx::shapes::Const::<K>));
//     let d_a_t = d_dev.tensor_from_vec(
//         a_t_data,
//         (
//             dfdx::shapes::Const::<B>,
//             dfdx::shapes::Const::<K>,
//             dfdx::shapes::Const::<M>,
//         ),
//     );
//     let d_b_t = d_dev.tensor_from_vec(
//         b_t_data,
//         (dfdx::shapes::Const::<K>, dfdx::shapes::Const::<N>),
//     );
//     let d_a_b = d_a
//         .clone()
//         .matmul(d_b.clone().permute::<_, dfdx::shapes::Axes2<1, 0>>());
//     let d_a_b_t = d_a.matmul(d_b_t.clone());
//     let d_a_t_b = d_a_t
//         .clone()
//         .permute::<_, dfdx::shapes::Axes3<0, 2, 1>>()
//         .matmul(d_b.permute::<_, dfdx::shapes::Axes2<1, 0>>());
//     let d_a_t_b_t = d_a_t
//         .permute::<_, dfdx::shapes::Axes3<0, 2, 1>>()
//         .matmul(d_b_t);

//     assert_close_precision(&a_b.data(), &d_a_b.as_vec(), 1);
//     assert_close_precision(&a_b_t.data(), &d_a_b_t.as_vec(), 1);
//     assert_close_precision(&a_t_b.data(), &d_a_t_b.as_vec(), 1);
//     assert_close_precision(&a_t_b_t.data(), &d_a_t_b_t.as_vec(), 1);
// }

// #[test]
// fn test_matmul_transpose() {
//     const M: usize = 1024; // Any
//     const K: usize = 16; // >= 16
//     const N: usize = 767; // >= 256, multiple of 256
//     let mut cx = Graph::new();
//     let mut rng = StdRng::seed_from_u64(0);

//     let a_data = random_vec_rng(M * K, &mut rng);
//     let a = cx.tensor::<R2<M, K>>().set(a_data.clone());
//     let b_data = random_vec_rng(K * N, &mut rng);
//     let b = cx.tensor::<R2<N, K>>().set(b_data.clone());
//     let a_t_data = random_vec_rng(K * M, &mut rng);
//     let a_t = cx.tensor::<R2<K, M>>().set(a_t_data.clone());
//     let b_t_data = random_vec_rng(K * N, &mut rng);
//     let b_t = cx.tensor::<R2<K, N>>().set(b_t_data.clone());

//     let mut a_b = a.matmul(b.permute()).retrieve();
//     let mut a_b_t = a.matmul(b_t).retrieve();
//     let mut a_t_b = a_t
//         .permute::<_, luminal::prelude::Axes2<1, 0>>()
//         .matmul(b.permute())
//         .retrieve();
//     let mut a_t_b_t = a_t
//         .permute::<_, luminal::prelude::Axes2<1, 0>>()
//         .matmul(b_t)
//         .retrieve();

//     cx.compile(
//         <(GenericCompiler, MetalCompiler<f16>)>::default(),
//         (&mut a_b, &mut a_b_t, &mut a_t_b, &mut a_t_b_t),
//     );
//     cx.execute();

//     let d_dev = Cpu::default();
//     let d_a = d_dev
//         .tensor_from_vec(a_data, (dfdx::shapes::Const::<M>, dfdx::shapes::Const::<K>))
//         .to_dtype::<f16>();
//     let d_b = d_dev
//         .tensor_from_vec(b_data, (dfdx::shapes::Const::<N>, dfdx::shapes::Const::<K>))
//         .to_dtype::<f16>();
//     let d_a_t = d_dev
//         .tensor_from_vec(
//             a_t_data,
//             (dfdx::shapes::Const::<K>, dfdx::shapes::Const::<M>),
//         )
//         .to_dtype::<f16>();
//     let d_b_t = d_dev
//         .tensor_from_vec(
//             b_t_data,
//             (dfdx::shapes::Const::<K>, dfdx::shapes::Const::<N>),
//         )
//         .to_dtype::<f16>();
//     let d_a_b = d_a.clone().matmul(d_b.clone().permute());
//     let d_a_b_t = d_a.matmul(d_b_t.clone());
//     let d_a_t_b = d_a_t
//         .clone()
//         .permute::<_, dfdx::shapes::Axes2<1, 0>>()
//         .matmul(d_b.permute());
//     let d_a_t_b_t = d_a_t
//         .permute::<_, dfdx::shapes::Axes2<1, 0>>()
//         .matmul(d_b_t);

//     assert_close(&a_b.data(), &d_a_b.to_dtype::<f32>().as_vec());
//     assert_close(&a_b_t.data(), &d_a_b_t.to_dtype::<f32>().as_vec());
//     assert_close(&a_t_b.data(), &d_a_t_b.to_dtype::<f32>().as_vec());
//     assert_close(&a_t_b_t.data(), &d_a_t_b_t.to_dtype::<f32>().as_vec());
// }

#[test]
fn test_relu_and_linear() {
    // Test single and batch, unoptimized and optimized
    let mut cx = Graph::new();
    let input_data = random_vec(32);
    let w1 = random_vec(32 * 64);
    let w2 = random_vec(32 * 64);
    let batch = cx
        .named_tensor::<R2<2, 32>>("Batch")
        .set(random_vec(32 * 2));
    let a = cx.named_tensor::<R1<32>>("Single").set(input_data.clone());

    let model: (Linear<32, 64>, ReLU, Linear<64, 32>) = InitModule::initialize(&mut cx);
    model.0.weight.set(w1.clone());
    model.2.weight.set(w2.clone());
    let mut b = model.forward(a).retrieve();
    let mut batch_out = model.forward(batch).retrieve();
    cx.execute();

    let unoptimized_b = b.data();
    let unoptimized_batch_out = batch_out.data();
    b.drop();
    batch_out.drop();
    cx.compile(
        <(GenericCompiler, MetalCompiler<f16>)>::default(),
        (&mut b, &mut batch_out),
    );
    cx.execute();

    assert_close_precision(&unoptimized_b, &b.data(), 2);
    assert_close_precision(&unoptimized_batch_out, &batch_out.data(), 2);

    // Test against dfdx
    let dev = Cpu::default();
    let mut model = <(
        dfdx::nn::modules::builders::UnbiasedLinear<32, 64>,
        dfdx::nn::modules::builders::ReLU,
        dfdx::nn::modules::builders::UnbiasedLinear<64, 32>,
    )>::build_on_device(&dev);
    // Set weights
    model.0.weight = dev
        .tensor_from_vec(w1, (dfdx::shapes::Const::<32>, dfdx::shapes::Const::<64>))
        .permute()
        .to_dtype::<f16>();
    model.2.weight = dev
        .tensor_from_vec(w2, (dfdx::shapes::Const::<64>, dfdx::shapes::Const::<32>))
        .permute()
        .to_dtype::<f16>();
    let a = dev
        .tensor_from_vec(input_data, (dfdx::shapes::Const::<32>,))
        .to_dtype::<f16>();
    let out = model.forward(a);

    assert_close_precision(&unoptimized_b, &out.to_dtype::<f32>().as_vec(), 2);
}

#[test]
fn test_rms_norm() {
    // Test single and batch, unoptimized and optimized
    let inp_data = random_vec(15 * 32);
    let weight_data = random_vec(32);
    let mut cx = Graph::new();
    let a = cx.tensor::<R2<15, 32>>().set(inp_data.clone());

    let model = RMSNorm::<32>::initialize(&mut cx);
    model.weight.set(weight_data.clone());
    let mut b = model.forward(a).retrieve();

    cx.compile(<(GenericCompiler, MetalCompiler<f16>)>::default(), &mut b);
    cx.execute();

    // Test against dfdx
    let dev = Cpu::default();
    let weight = dev
        .tensor_from_vec(weight_data, (dfdx::shapes::Const::<32>,))
        .to_dtype::<f16>();
    let a = dev
        .tensor_from_vec(
            inp_data,
            (dfdx::shapes::Const::<15>, dfdx::shapes::Const::<32>),
        )
        .to_dtype::<f16>();
    let var_f32 = a.clone().square().mean::<_, dfdx::shapes::Axis<1>>();
    let std_f32 = (var_f32 + 1e-6).sqrt();
    let x_f32 = a / std_f32.broadcast();
    let out = weight.broadcast() * x_f32.to_dtype::<f16>();

    assert_close(&b.data(), &out.to_dtype::<f32>().as_vec());
}

#[test]
fn test_layer_norm() {
    let mut cx = Graph::new();
    let a_data = random_vec(15 * 16 * 32);
    let a = cx.tensor::<R3<15, 16, 32>>().set(a_data.clone());
    let mut b = a.layer_norm::<0, _>(1e-5).retrieve();
    let mut c = a.layer_norm::<2, _>(1e-5).retrieve();
    cx.compile(
        <(GenericCompiler, MetalCompiler<f16>)>::default(),
        (&mut b, &mut c),
    );
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev.tensor_from_vec(
        a_data,
        (
            dfdx::shapes::Const::<15>,
            dfdx::shapes::Const::<16>,
            dfdx::shapes::Const::<32>,
        ),
    );
    let d_b = d_a.clone().normalize::<dfdx::shapes::Axis<0>>(1e-5);
    let d_c = d_a.normalize::<dfdx::shapes::Axis<2>>(1e-5);

    assert_close_precision(&b.data(), &d_b.as_vec(), 2);
    assert_close_precision(&c.data(), &d_c.as_vec(), 2);
}

#[test]
fn test_transformer_encoder_block() {
    let mut cx = Graph::new();
    let model: luminal::nn::transformer::encoder::TransformerEncoderBlock<32, 64, 1> =
        InitModule::initialize(&mut cx);
    let w_k_weight = random_vec(32 * 32);
    model.attention.w_k.weight.set(w_k_weight.clone());
    let w_q_weight = random_vec(32 * 32);
    model.attention.w_q.weight.set(w_q_weight.clone());
    let w_v_weight = random_vec(32 * 32);
    model.attention.w_v.weight.set(w_v_weight.clone());
    let w_o_weight = random_vec(32 * 32);
    model.attention.w_o.weight.set(w_o_weight.clone());
    let ff_0_weight = random_vec(32 * 64);
    model.ff.0.weight.set(ff_0_weight.clone());
    let ff_1_weight = random_vec(64 * 32);
    model.ff.2.weight.set(ff_1_weight.clone());

    let a_data = random_vec(2 * 32);
    let a = cx
        .tensor::<(Dyn<'b'>, Dyn<'a'>, luminal::prelude::Const<32>)>()
        .set_dyn(a_data.clone(), &[1, 2, 3])
        .keep();
    cx.keep_tensors(state_dict(&model));
    let mut b = model.forward(a).retrieve();
    cx.execute();
    let unopt_b = b.data();
    b.drop();

    cx.compile(<(GenericCompiler, MetalCompiler<f16>)>::default(), &mut b);
    cx.execute();
    assert_close_precision(&unopt_b, &b.data(), 2);

    let d_dev = Cpu::default();
    let mut d_model: dfdx::nn::modules::TransformerEncoderBlock<32, 1, 64, f32, Cpu> =
        d_dev
            .build_module::<dfdx::nn::modules::builders::TransformerEncoderBlock<32, 1, 64>, f32>();
    d_model.self_attn.w_k.bias.copy_from(&[0.; 32]);
    d_model.self_attn.w_v.bias.copy_from(&[0.; 32]);
    d_model.self_attn.w_q.bias.copy_from(&[0.; 32]);
    d_model.self_attn.w_o.bias.copy_from(&[0.; 32]);
    d_model.self_attn.w_o.weight = d_dev
        .tensor_from_vec(
            w_o_weight,
            (dfdx::shapes::Const::<32>, dfdx::shapes::Const::<32>),
        )
        .permute();
    d_model.self_attn.w_k.weight = d_dev
        .tensor_from_vec(
            w_k_weight,
            (dfdx::shapes::Const::<32>, dfdx::shapes::Const::<32>),
        )
        .permute();
    d_model.self_attn.w_q.weight = d_dev
        .tensor_from_vec(
            w_q_weight,
            (dfdx::shapes::Const::<32>, dfdx::shapes::Const::<32>),
        )
        .permute();
    d_model.self_attn.w_v.weight = d_dev
        .tensor_from_vec(
            w_v_weight,
            (dfdx::shapes::Const::<32>, dfdx::shapes::Const::<32>),
        )
        .permute();
    d_model.ff.0 .0.weight = d_dev
        .tensor_from_vec(
            ff_0_weight,
            (dfdx::shapes::Const::<32>, dfdx::shapes::Const::<64>),
        )
        .permute();
    d_model.ff.0 .0.bias = d_dev.tensor_from_vec(vec![0.; 64], (dfdx::shapes::Const::<64>,));
    d_model.ff.0 .2.weight = d_dev
        .tensor_from_vec(
            ff_1_weight,
            (dfdx::shapes::Const::<64>, dfdx::shapes::Const::<32>),
        )
        .permute();
    d_model.ff.0 .2.bias = d_dev.tensor_from_vec(vec![0.; 32], (dfdx::shapes::Const::<32>,));
    d_model.norm1.gamma = d_dev.tensor_from_vec(vec![1.; 32], (dfdx::shapes::Const::<32>,));
    d_model.norm2.gamma = d_dev.tensor_from_vec(vec![1.; 32], (dfdx::shapes::Const::<32>,));
    d_model.norm1.epsilon = 1e-5;
    d_model.norm2.beta = d_dev.tensor_from_vec(vec![0.; 32], (dfdx::shapes::Const::<32>,));
    d_model.norm1.beta = d_dev.tensor_from_vec(vec![0.; 32], (dfdx::shapes::Const::<32>,));
    d_model.norm2.epsilon = 1e-5;
    let d_a = d_dev.tensor_from_vec(
        a_data,
        (dfdx::shapes::Const::<2>, dfdx::shapes::Const::<32>),
    );
    let d_b = d_model.forward(d_a);

    assert_close_precision(&b.data(), &d_b.as_vec(), 2);
}

#[test]
fn test_common_buffer() {
    let data = random_vec(32);
    let mut cx = Graph::new();
    let a = cx.tensor::<R1<32>>();
    a.set(data.clone());
    let a1 = cx.tensor::<R1<32>>();
    a1.set(data.clone());
    let exped = a * a1;
    let mut b = exped.log2().retrieve();
    let mut c = exped.sin().retrieve();

    cx.compile(MetalCompiler::<f16>::default(), (&mut b, &mut c));
    cx.execute();
}

#[test]
fn test_embedding() {
    let mut cx = Graph::new();
    let batch = cx
        .named_tensor::<R2<2, 3>>("Batch")
        .set(vec![1.0, 0.0, 2.0, 1.0, 0.0, 1.0])
        .keep();
    let a = cx
        .named_tensor::<R1<3>>("Single")
        .set(vec![1.0, 0.0, 1.0])
        .keep();

    let model: luminal::nn::embedding::Embedding<3, 4> = InitModule::initialize(&mut cx);
    model
        .weight
        .set(vec![1.1, 2., 3., 1., 2., 3., 14., 2., 33., 1., 2., 3.]);
    let mut b = model.forward(a).retrieve();
    let mut batch_out = model.forward(batch).retrieve();

    cx.compile(MetalCompiler::<f16>::default(), (&mut b, &mut batch_out));
    cx.execute();

    let d_dev = Cpu::default();
    let mut d_model: modules::Embedding<3, 4, f32, Cpu> =
        <dfdx::nn::modules::builders::Embedding<3, 4>>::build_on_device(&d_dev);
    d_model.weight = d_dev.tensor_from_vec(
        vec![1.1, 2., 3., 1., 2., 3., 14., 2., 33., 1., 2., 3.],
        (dfdx::shapes::Const::<3>, dfdx::shapes::Const::<4>),
    );
    let d_a = d_dev.tensor_from_vec(vec![1, 0, 1], (dfdx::shapes::Const::<3>,));
    let d_batch = d_dev.tensor_from_vec(
        vec![1, 0, 2, 1, 0, 1],
        (dfdx::shapes::Const::<2>, dfdx::shapes::Const::<3>),
    );

    let d_b = d_model.forward(d_a);
    let d_batch_out = d_model.forward(d_batch);

    assert_close(&b.data(), &d_b.as_vec());
    assert_close(&batch_out.data(), &d_batch_out.as_vec());
}

#[test]
fn test_slice() {
    let data = random_vec(256);
    let mut cx = Graph::new();
    let a = cx.tensor::<R1<256>>().set(data.clone());
    let mut c: GraphTensor<R1<20>> = a
        .slice((..Expression::from(20),))
        .realize()
        .contiguous()
        .retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut c);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev
        .tensor_from_vec(data, (dfdx::shapes::Const::<256>,))
        .to_dtype::<f16>();
    let d_c = d_a.slice((..20,)).to_dtype::<f32>();

    assert_exact(&c.data(), &d_c.as_vec());
}

#[test]
fn test_pad() {
    // Pad a 8x2 mat to 10x4
    let data = random_vec(8 * 2);
    let mut cx = Graph::new();
    let a = cx.tensor::<R2<8, 2>>().set(data.clone());
    let mut c = a
        .pad::<R2<10, 4>, _, _>(&[(0, 2), (0, 2)])
        .contiguous()
        .retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut c);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev.tensor_from_vec(data, (8, 2)).to_dtype::<f16>();
    // There is no pad function in dfdx, so we concat with zero tensors
    let d_b = (d_a, d_dev.zeros_like(&(2, 2))).concat_along(dfdx::shapes::Axis::<0>);
    let d_c = (d_b, d_dev.zeros_like(&(10, 2))).concat_along(dfdx::shapes::Axis::<1>);

    assert_exact(&c.data(), &d_c.to_dtype::<f32>().as_vec());
}

#[test]
fn test_pad_contig() {
    let m = 13;
    let k = 24;
    let mut cx = Graph::new();
    let mut rng = StdRng::seed_from_u64(0);
    let a_data = random_vec_rng(m * k, &mut rng);
    let mut a = cx
        .tensor::<(Dyn<'M'>, Dyn<'K'>)>()
        .set_dyn(a_data, &[m, k])
        .retrieve();
    let mut b: GraphTensor<(Dyn<'M'>, Dyn<'K'>)> = a
        .pad(&[(0, 0.into()), (0, Expression::from(16) - 'K')])
        .contiguous()
        .retrieve();
    let mut c: GraphTensor<(Dyn<'M'>, Dyn<'K'>)> =
        (a.slice((.., ..Expression::from(k))).realize() / 1.0).retrieve();

    cx.compile(MetalCompiler::<f16>::default(), (&mut a, &mut b, &mut c));
    cx.execute();

    // Close because b and c are going through 16 bits, while a is not
    assert_close(&a.data(), &b.data());
    assert_close(&a.data(), &c.data());
}

#[test]
fn test_movement() {
    let data = random_vec(32);
    let mut cx = Graph::new();
    let a = cx.tensor::<R1<32>>().set(data.clone());
    let b: GraphTensor<R1<42>> = a.pad(&[(0, 10)]).contiguous().retrieve();
    let mut c: GraphTensor<R1<25>> = b
        .slice((..Expression::from(25),))
        .realize()
        .contiguous()
        .retrieve();

    cx.compile(MetalCompiler::<f16>::default(), &mut c);
    cx.execute();

    let d_dev = Cpu::default();
    let d_a = d_dev
        .tensor_from_vec(data, (dfdx::shapes::Const::<32>,))
        .to_dtype::<f16>();
    let d_c = d_a.slice((..25,)).to_dtype::<f32>();

    assert_exact(&c.data(), &d_c.as_vec());
}
