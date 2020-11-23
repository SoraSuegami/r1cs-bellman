use std::collections::{HashSet,BTreeMap};
use std::iter::FromIterator;

use bellman::{Circuit, ConstraintSystem, LinearCombination, SynthesisError, Variable};
use bellman::Index::{Aux, Input};
use bellman::SynthesisError::{AssignmentMissing};
use pairing::Engine;
use r1cs::{Constraint, Element, Expression, Field, Gadget, Wire};

use ff::PrimeField;

pub struct WrappedCircuit<F: Field, E: Engine> {
    pub gadget: Gadget<F>,
    pub witness_map: BTreeMap<u32,E::Fr>,
    pub public_inputs: Vec<Wire>,
    pub convert_field: fn(&Element<F>) -> E::Fr,
}

impl<F: Field, E: Engine> Circuit<E::Fr> for WrappedCircuit<F, E> {
    fn synthesize<CS: ConstraintSystem<E::Fr>>(self, cs: &mut CS) -> Result<(), SynthesisError> {
        let WrappedCircuit { gadget, witness_map, public_inputs, convert_field } = self;
        let public_inputs = HashSet::from_iter(public_inputs);
        let mut i=0;
        for constraint in gadget.constraints {
            let Constraint { a, b, c } = constraint;
            let a_lc = convert_lc::<F, E, CS>(cs, a, convert_field, &witness_map, &public_inputs);
            let b_lc = convert_lc::<F, E, CS>(cs, b, convert_field, &witness_map, &public_inputs);
            let c_lc = convert_lc::<F, E, CS>(cs, c, convert_field, &witness_map, &public_inputs);
            cs.enforce(
                || format!("generated by r1cs-bellman at {}", i),
                |_| a_lc,
                |_| b_lc,
                |_| c_lc,
            );
            i += 1;
        }
        Ok(())
    }
}

fn convert_lc<F: Field, E: Engine, CS: ConstraintSystem<E::Fr>>(
    cs: &mut CS,
    exp: Expression<F>,
    convert_field: fn(&Element<F>) -> E::Fr,
    witness_map: &BTreeMap<u32,E::Fr>,
    public_inputs: &HashSet<Wire>
) -> LinearCombination<E::Fr> {
    // This is inefficient, but bellman doesn't expose a LinearCombination constructor taking an
    // entire variable/coefficient map, so we have to build one up with repeated addition.
    let mut sum = LinearCombination::zero();
    for (wire, coeff) in exp.coefficients() {
        let fr = convert_field(coeff);
        let var = convert_wire::<E,CS>(cs, *wire, witness_map, public_inputs);
        sum = sum + (fr, var);
    }
    sum
}

fn convert_wire<E: Engine, CS: ConstraintSystem<E::Fr>>(
    cs: &mut CS,
    wire: Wire,
    witness_map: &BTreeMap<u32,E::Fr>,
    public_inputs: &HashSet<Wire>
) -> Variable {
    let wire_index = wire.index;
    let witness = witness_map.get(&wire_index);
    let is_public = public_inputs.contains(&wire);
    
    match witness {
        Some(wtns) => {
            if is_public {
                cs.alloc_input(|| "public input", || Ok(*wtns)).unwrap()
            } else {
                cs.alloc(|| "private input", || Ok(*wtns)).unwrap()
            }
        }
        None => {
            if is_public {
                cs.alloc_input(|| "public input", || Ok(E::Fr::from_str("0").unwrap())).unwrap()
            } else {
                cs.alloc(|| "private input", || Ok(E::Fr::from_str("0").unwrap())).unwrap()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bellman::groth16::{create_random_proof, generate_random_parameters, prepare_verifying_key, Proof, verify_proof};
    use num::{BigUint, Integer, One, ToPrimitive};
    use bls12_381::{Bls12};
    use ff::PrimeField;
    use pairing::Engine;
    use r1cs::{Bls12_381, Element, Gadget, GadgetBuilder, Expression, Wire};
    use rand::thread_rng;
    use std::collections::{BTreeMap};

    use crate::WrappedCircuit;

    #[test]
    fn valid_proof() {
        let rng = &mut thread_rng();

        // Generate random parameters.
        let empty_map = BTreeMap::<u32,<Bls12 as Engine>::Fr>::new();
        let circuit = build_circuit(empty_map);
        let params = generate_random_parameters::<Bls12, _, _>(circuit, rng).unwrap();
        let pvk = prepare_verifying_key(&params.vk);

        // Generate a random proof.
        let mut witness_map = BTreeMap::<u32,<Bls12 as Engine>::Fr>::new();
        //1*6 = 6
        witness_map.insert(1,convert_bls12_381(&Element::from(1u8)));
        witness_map.insert(2,convert_bls12_381(&Element::from(6u8)));
        witness_map.insert(3,convert_bls12_381(&Element::from(6u8)));
        let circuit = build_circuit(witness_map);
        let proof = create_random_proof(circuit, &params, rng).unwrap();

        // Serialize and deserialize the proof.
        let mut proof_out = vec![];
        proof.write(&mut proof_out).unwrap();
        let proof = Proof::read(&proof_out[..]).unwrap();

        // Verify the proof.
        let public_inputs = &[convert_bls12_381(&Element::from(6u8))];
        verify_proof(&pvk, &proof, public_inputs).unwrap();
        assert!(verify_proof(&pvk, &proof, public_inputs).is_ok());
    }

    #[test]
    fn invalid_proof() {
        let rng = &mut thread_rng();

        // Generate random parameters.
        let empty_map = BTreeMap::<u32,<Bls12 as Engine>::Fr>::new();
        let circuit = build_circuit(empty_map);
        let params = generate_random_parameters::<Bls12, _, _>(circuit, rng).unwrap();
        let pvk = prepare_verifying_key(&params.vk);

        // Generate a random proof.
        let mut witness_map = BTreeMap::<u32,<Bls12 as Engine>::Fr>::new();
        // 2*6 != 6
        witness_map.insert(1,convert_bls12_381(&Element::from(2u8)));
        witness_map.insert(2,convert_bls12_381(&Element::from(6u8)));
        witness_map.insert(3,convert_bls12_381(&Element::from(6u8)));
        let circuit = build_circuit(witness_map);
        let proof = create_random_proof(circuit, &params, rng).unwrap();

        // Serialize and deserialize the proof.
        let mut proof_out = vec![];
        proof.write(&mut proof_out).unwrap();
        let proof = Proof::read(&proof_out[..]).unwrap();

        // Verify the proof.
        let public_inputs = &[convert_bls12_381(&Element::from(6u8))];
        assert!(verify_proof(&pvk, &proof, public_inputs).is_err());
    }

    fn build_circuit(witness_map: BTreeMap<u32,<Bls12 as Engine>::Fr>) -> WrappedCircuit<r1cs::Bls12_381, bls12_381::Bls12> {
        let mut builder = GadgetBuilder::<Bls12_381>::new();
        let x = builder.wire();
        println!("x wire:{}",x.index);
        let y = builder.wire();
        let z = builder.wire();
        builder.assert_product(&Expression::from(&x), &Expression::from(&y), &Expression::from(&z));
        let gadget = builder.build();
        WrappedCircuit {
            gadget,
            witness_map,
            public_inputs: vec![z],
            convert_field: convert_bls12_381,
        }
    }

    fn convert_bls12_381(n: &Element<r1cs::Bls12_381>) -> <Bls12 as Engine>::Fr {
        let n = n.to_biguint();
        // Bls12::Fr::FrRepr's chunks are little endian.
        let u64_size = BigUint::one() << 64;
        let chunks = [
            n.mod_floor(&u64_size).to_u64().unwrap(),
            (n >> 64).mod_floor(&u64_size).to_u64().unwrap(),
            (n >> 64 * 2).mod_floor(&u64_size).to_u64().unwrap(),
            (n >> 64 * 3).mod_floor(&u64_size).to_u64().unwrap(),
        ];
        <Bls12 as Engine>::Fr::from_repr(bls12_381::Scalar::from_raw(chunks).to_bytes()).unwrap()
    }
}