use support::{
	decl_module, decl_storage, decl_event, ensure, StorageValue, StorageMap,
	Parameter, traits::{Randomness, Currency, ExistenceRequirement}
};
use sp_runtime::traits::{SimpleArithmetic, Bounded, Member};
use codec::{Encode, Decode};
use runtime_io::hashing::blake2_128;
use system::ensure_signed;
use rstd::result;
use crate::linked_item::{LinkedList, LinkedItem};


pub trait Trait: system::Trait {
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;
	type KittyIndex: Parameter + Member + SimpleArithmetic + Bounded + Default + Copy;
	type Currency: Currency<Self::AccountId>;
	type Randomness: Randomness<Self::Hash>;
}

type BalanceOf<T> = <<T as Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::Balance;

//const MIN_BREED_AGE:u32 = 2000;//从小猫被创建的区块开始，至少经过2000区块后，才可以生育
//const MAX_BREED_AGE:u32 = 10000;//从小猫被创建的区块开始，至少经过100000区块后，才可以生育
//const MAX_AGE:u32 = 100000;//从小猫被创建的区块开始，超过100000区块后，小猫将死亡


#[derive(Encode, Decode)]
pub struct Kitty<BlockNumber> {
	pub dna: [u8; 16], 
	///小猫被创建时的区块高度，【小猫的年龄】 = 当前区块数 - 被创建的区块数
	pub create_block_number: BlockNumber 
}

type KittyLinkedItem<T> = LinkedItem<<T as Trait>::KittyIndex>;
type OwnedKittiesList<T> = LinkedList<OwnedKitties<T>, <T as system::Trait>::AccountId, <T as Trait>::KittyIndex>;

decl_storage! {
	trait Store for Module<T: Trait> as Kitties {
		/// Stores all the kitties, key is the kitty id / index
		pub Kitties get(fn kitties): map T::KittyIndex => Option<Kitty<T::BlockNumber>>;
		/// Stores the total number of kitties. i.e. the next kitty index
		pub KittiesCount get(fn kitties_count): T::KittyIndex;

		pub OwnedKitties get(fn owned_kitties): map (T::AccountId, Option<T::KittyIndex>) => Option<KittyLinkedItem<T>>;

		/// Get kitty owner
		pub KittyOwners get(fn kitty_owner): map T::KittyIndex => Option<T::AccountId>;
		/// Get kitty price. None means not for sale.
		pub KittyPrices get(fn kitty_price): map T::KittyIndex => Option<BalanceOf<T>>;

		//年龄的相关参数定义：
		pub MinBreedAge: u32;//可以生育的，最小年龄
		pub MaxBreedAge: u32;//可以生育的，最大年龄
		pub MaxAge: u32;//最大年龄，超过该年龄，即为死亡
		pub Owner: T::AccountId;//管理员
		pub Initial: bool;//是否初始化，只有初始化后，才可以使用所有功能

	}
}

decl_event!(
	pub enum Event<T> where
		<T as system::Trait>::AccountId,
		<T as Trait>::KittyIndex,
		Balance = BalanceOf<T>,
	{
		/// A kitty is created. (owner, kitty_id)
		Created(AccountId, KittyIndex),
		/// A kitty is transferred. (from, to, kitty_id)
		Transferred(AccountId, AccountId, KittyIndex),
		/// A kitty is available for sale. (owner, kitty_id, price)
		Ask(AccountId, KittyIndex, Option<Balance>),
		/// A kitty is sold. (from, to, kitty_id, price)
		Sold(AccountId, AccountId, KittyIndex, Balance),
	}
);

decl_module! {
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		fn deposit_event() = default;
		//初始化函数，只能执行一次
		pub fn init(origin, min_breed_age: u32, max_breed_age: u32, max_age: u32) {
			ensure!(!<Initial>::get(), "Runtime has been already initialized");
			ensure!(min_breed_age > 0 && min_breed_age <= max_breed_age && max_breed_age < max_age, "Breed limitation ages not valid");

			let sender = ensure_signed(origin)?;
			<MinBreedAge>::put(min_breed_age);
			<MaxBreedAge>::put(max_breed_age);
			<MaxAge>::put(max_age);
			<Owner<T>>::put(sender);
			<Initial>::put(true);
		}

		pub fn create(origin) {
			ensure!(<Initial>::get(), "Runtime has not been initialized");
			let sender = ensure_signed(origin)?;
			let kitty_id = Self::next_kitty_id()?;

			// Generate a random 128bit value
			let dna = Self::random_value(&sender);
			// Create and store kitty
			Self::insert_kitty(&sender, kitty_id, dna);

			Self::deposit_event(RawEvent::Created(sender, kitty_id));
		}

		/// Breed kitties
		pub fn breed(origin, kitty_id_1: T::KittyIndex, kitty_id_2: T::KittyIndex) {
			ensure!(<Initial>::get(), "Runtime has not been initialized");
			ensure!(Self::check_liveness(kitty_id_1), "Kitty1 is dead, cannot breed anymore");
			ensure!(Self::check_liveness(kitty_id_2), "Kitty2 is dead, cannot breed anymore");

			let sender = ensure_signed(origin)?;
			let new_kitty_id = Self::do_breed(&sender, kitty_id_1, kitty_id_2)?;

			Self::deposit_event(RawEvent::Created(sender, new_kitty_id));
		}

		/// Transfer a kitty to new owner
 		pub fn transfer(origin, to: T::AccountId, kitty_id: T::KittyIndex) {
			ensure!(<Initial>::get(), "Runtime has not been initialized");
			ensure!(Self::check_liveness(kitty_id), "Kitty is dead, cannot breed anymore");
 			let sender = ensure_signed(origin)?;

  			ensure!(<OwnedKitties<T>>::exists(&(sender.clone(), Some(kitty_id))), "Only owner can transfer kitty");

			Self::do_transfer(&sender, &to, kitty_id);

			Self::deposit_event(RawEvent::Transferred(sender, to, kitty_id));
		}

		/// Set a price for a kitty for sale
		/// None to delist the kitty
		pub fn ask(origin, kitty_id: T::KittyIndex, price: Option<BalanceOf<T>>) {
			ensure!(<Initial>::get(), "Runtime has not been initialized");
			ensure!(Self::check_liveness(kitty_id), "Kitty is dead, cannot ask for sale");

			let sender = ensure_signed(origin)?;
			ensure!(<OwnedKitties<T>>::exists(&(sender.clone(), Some(kitty_id))), "Only owner can set price for kitty");

			if let Some(ref price) = price {
				<KittyPrices<T>>::insert(kitty_id, price);
			} else {
				<KittyPrices<T>>::remove(kitty_id);
			}

			Self::deposit_event(RawEvent::Ask(sender, kitty_id, price));
		}

		pub fn buy(origin, kitty_id: T::KittyIndex, price: BalanceOf<T>) {
			ensure!(<Initial>::get(), "Runtime has not been initialized");
			let isLive = Self::check_liveness(kitty_id);
			if !isLive {//如果小猫死亡，需要从挂单中删除
				<KittyPrices<T>>::remove(kitty_id);
			}
			ensure!(isLive, "Kitty is dead, cannot sale anymore");

			let sender = ensure_signed(origin)?;

			let owner = Self::kitty_owner(kitty_id);
			ensure!(owner.is_some(), "Kitty does not exist");
			let owner = owner.unwrap();

			let kitty_price = Self::kitty_price(kitty_id);
			ensure!(kitty_price.is_some(), "Kitty not for sale");

			let kitty_price = kitty_price.unwrap();
			ensure!(price >= kitty_price, "Price is too low");

			T::Currency::transfer(&sender, &owner, kitty_price, ExistenceRequirement::KeepAlive)?;

			<KittyPrices<T>>::remove(kitty_id);

			Self::do_transfer(&owner, &sender, kitty_id);

			Self::deposit_event(RawEvent::Sold(owner, sender, kitty_id, kitty_price));
		}

		//更新可生育的最小年龄
		pub fn update_min_breed_age(origin, min_breed_age: u32) {
			ensure!(<Initial>::get(), "Runtime has not been initialized");
			ensure!(min_breed_age > 0 && min_breed_age <= <MaxBreedAge>::get(), "Min breed age is not valid");

			let sender = ensure_signed(origin)?;

			ensure!(sender == <Owner<T>>::get(), "You are not the owner.");
			<MinBreedAge>::put(min_breed_age);
		}
		//更新可生育的最大年龄
		pub fn update_max_breed_age(origin, max_breed_age: u32) {
			ensure!(<Initial>::get(), "Runtime has not been initialized");
			ensure!(<MinBreedAge>::get() <= max_breed_age && max_breed_age < <MaxAge>::get(), "Max bredd age is not valid");

			let sender = ensure_signed(origin)?;

			ensure!(sender == <Owner<T>>::get(), "You are not the owner.");
			<MaxBreedAge>::put(max_breed_age);
		}
		//更新最大年龄
		pub fn update_max_age(origin, max_age: u32) {
			ensure!(<Initial>::get(), "Runtime has not been initialized");
			ensure!(<MaxBreedAge>::get() <= max_age, "Max age is not valid");

			let sender = ensure_signed(origin)?;

			ensure!(sender == <Owner<T>>::get(), "You are not the owner.");
			<MaxAge>::put(max_age);
		}
		//更新admin
		pub fn update_owner(origin, new_owner: T::AccountId) {
			ensure!(<Initial>::get(), "Runtime has not been initialized");
			let sender = ensure_signed(origin)?;

			ensure!(sender == <Owner<T>>::get(), "You are not the owner.");
			<Owner<T>>::put(new_owner);
		}
	}
}

fn combine_dna(dna1: u8, dna2: u8, selector: u8) -> u8 {
	((selector & dna1) | (!selector & dna2))
}

impl<T: Trait> Module<T> {
	fn random_value(sender: &T::AccountId) -> [u8; 16] {
		let payload = (
			T::Randomness::random_seed(),
			&sender,
			<system::Module<T>>::extrinsic_index(),
			<system::Module<T>>::block_number(),
		);
		payload.using_encoded(blake2_128)
	}

	fn next_kitty_id() -> result::Result<T::KittyIndex, &'static str> {
		let kitty_id = Self::kitties_count();
		if kitty_id == T::KittyIndex::max_value() {
			return Err("Kitties count overflow");
		}
		Ok(kitty_id)
	}

	fn insert_owned_kitty(owner: &T::AccountId, kitty_id: T::KittyIndex) {
		<OwnedKittiesList<T>>::append(owner, kitty_id);
	}

	fn insert_kitty(owner: &T::AccountId, kitty_id: T::KittyIndex, dna: [u8; 16]) {
		/// 记录小猫被创建时的区块数，以此计算小猫的年龄
		let block_number = <system::Module<T>>::block_number();
		let kitty = Kitty{
			dna: dna,
			create_block_number: block_number
		};

		<Kitties<T>>::insert(kitty_id, kitty);
		<KittiesCount<T>>::put(kitty_id + 1.into());
		<KittyOwners<T>>::insert(kitty_id, owner.clone());

		Self::insert_owned_kitty(owner, kitty_id);

	}

	fn do_breed(sender: &T::AccountId, kitty_id_1: T::KittyIndex, kitty_id_2: T::KittyIndex) -> result::Result<T::KittyIndex, &'static str> {
		let kitty1 = Self::kitties(kitty_id_1);
		let kitty2 = Self::kitties(kitty_id_2);

		ensure!(kitty1.is_some(), "Invalid kitty_id_1");
		ensure!(kitty2.is_some(), "Invalid kitty_id_2");
		ensure!(kitty_id_1 != kitty_id_2, "Needs different parent");
		ensure!(Self::kitty_owner(&kitty_id_1).map(|owner| owner == *sender).unwrap_or(false), "Not onwer of kitty1");
 		ensure!(Self::kitty_owner(&kitty_id_2).map(|owner| owner == *sender).unwrap_or(false), "Not owner of kitty2");

		let kitty1 = kitty1.unwrap();
		let kitty2 = kitty2.unwrap();
		let block_number = <system::Module<T>>::block_number();

		let age1 = block_number - kitty1.create_block_number.into();
		let age2 = block_number - kitty2.create_block_number.into();

		ensure!( age1 >= <MinBreedAge>::get().into() && age1 <= <MaxBreedAge>::get().into(), "kitty1's age is not allowed to breed.");
		ensure!( age2 >= <MinBreedAge>::get().into() && age2 <= <MaxBreedAge>::get().into(), "kitty2's age is not allowed to breed.");

		let kitty_id = Self::next_kitty_id()?;

		let kitty1_dna = kitty1.dna;
		let kitty2_dna = kitty2.dna;

		// Generate a random 128bit value
		let selector = Self::random_value(&sender);
		let mut new_dna = [0u8; 16];

		// Combine parents and selector to create new kitty
		for i in 0..kitty1_dna.len() {
			new_dna[i] = combine_dna(kitty1_dna[i], kitty2_dna[i], selector[i]);
		}

		Self::insert_kitty(sender, kitty_id, new_dna);

		Ok(kitty_id)
	}

	fn do_transfer(from: &T::AccountId, to: &T::AccountId, kitty_id: T::KittyIndex)  {
 		<OwnedKittiesList<T>>::remove(&from, kitty_id);
 		<OwnedKittiesList<T>>::append(&to, kitty_id);
 		<KittyOwners<T>>::insert(kitty_id, to);
 	}
	fn check_liveness(kitty_id: T::KittyIndex) -> bool{
		let block_number = <system::Module<T>>::block_number();
		let kitty = Self::kitties(kitty_id).unwrap();
		if block_number - kitty.create_block_number < <MaxAge>::get().into() {
			return true;
		}
		return false;
	}
}

/// Tests for Kitties module
#[cfg(test)]
mod tests {
	use super::*;

	use primitives::H256;
	use support::{impl_outer_origin, assert_ok, parameter_types, weights::Weight};
	use sp_runtime::{
		traits::{BlakeTwo256, IdentityLookup}, testing::Header, Perbill,
	};

	impl_outer_origin! {
		pub enum Origin for Test {}
	}

	// For testing the module, we construct most of a mock runtime. This means
	// first constructing a configuration type (`Test`) which `impl`s each of the
	// configuration traits of modules we want to use.
	#[derive(Clone, Eq, PartialEq, Debug)]
	pub struct Test;
	parameter_types! {
		pub const BlockHashCount: u64 = 250;
		pub const MaximumBlockWeight: Weight = 1024;
		pub const MaximumBlockLength: u32 = 2 * 1024;
		pub const AvailableBlockRatio: Perbill = Perbill::from_percent(75);
	}
	impl system::Trait for Test {
		type Origin = Origin;
		type Call = ();
		type Index = u64;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type MaximumBlockWeight = MaximumBlockWeight;
		type MaximumBlockLength = MaximumBlockLength;
		type AvailableBlockRatio = AvailableBlockRatio;
		type Version = ();
	}
	parameter_types! {
		pub const ExistentialDeposit: u64 = 0;
		pub const TransferFee: u64 = 0;
		pub const CreationFee: u64 = 0;
	}
	impl balances::Trait for Test {
		type Balance = u64;
		type OnFreeBalanceZero = ();
		type OnNewAccount = ();
		type Event = ();
		type TransferPayment = ();
		type DustRemoval = ();
		type ExistentialDeposit = ExistentialDeposit;
		type TransferFee = TransferFee;
		type CreationFee = CreationFee;
	}
	impl Trait for Test {
		type KittyIndex = u32;
		type Currency = balances::Module<Test>;
		type Randomness = randomness_collective_flip::Module<Test>;
		type Event = ();
	}
	type OwnedKittiesTest = OwnedKitties<Test>;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	fn new_test_ext() -> runtime_io::TestExternalities {
		system::GenesisConfig::default().build_storage::<Test>().unwrap().into()
	}

	#[test]
	fn owned_kitties_can_append_values() {
		new_test_ext().execute_with(|| {
			OwnedKittiesList::<Test>::append(&0, 1);

			assert_eq!(OwnedKittiesTest::get(&(0, None)), Some(KittyLinkedItem::<Test> {
				prev: Some(1),
				next: Some(1),
			}));

			assert_eq!(OwnedKittiesTest::get(&(0, Some(1))), Some(KittyLinkedItem::<Test> {
				prev: None,
				next: None,
			}));

			OwnedKittiesList::<Test>::append(&0, 2);

			assert_eq!(OwnedKittiesTest::get(&(0, None)), Some(KittyLinkedItem::<Test> {
				prev: Some(2),
				next: Some(1),
			}));

			assert_eq!(OwnedKittiesTest::get(&(0, Some(1))), Some(KittyLinkedItem::<Test> {
				prev: None,
				next: Some(2),
			}));

			assert_eq!(OwnedKittiesTest::get(&(0, Some(2))), Some(KittyLinkedItem::<Test> {
				prev: Some(1),
				next: None,
			}));

			OwnedKittiesList::<Test>::append(&0, 3);

			assert_eq!(OwnedKittiesTest::get(&(0, None)), Some(KittyLinkedItem::<Test> {
				prev: Some(3),
				next: Some(1),
			}));

			assert_eq!(OwnedKittiesTest::get(&(0, Some(1))), Some(KittyLinkedItem::<Test> {
				prev: None,
				next: Some(2),
			}));

			assert_eq!(OwnedKittiesTest::get(&(0, Some(2))), Some(KittyLinkedItem::<Test> {
				prev: Some(1),
				next: Some(3),
			}));

			assert_eq!(OwnedKittiesTest::get(&(0, Some(3))), Some(KittyLinkedItem::<Test> {
				prev: Some(2),
				next: None,
			}));
		});
	}

	#[test]
	fn owned_kitties_can_remove_values() {
		new_test_ext().execute_with(|| {
			OwnedKittiesList::<Test>::append(&0, 1);
			OwnedKittiesList::<Test>::append(&0, 2);
			OwnedKittiesList::<Test>::append(&0, 3);

			OwnedKittiesList::<Test>::remove(&0, 2);

			assert_eq!(OwnedKittiesTest::get(&(0, None)), Some(KittyLinkedItem::<Test> {
				prev: Some(3),
				next: Some(1),
			}));

			assert_eq!(OwnedKittiesTest::get(&(0, Some(1))), Some(KittyLinkedItem::<Test> {
				prev: None,
				next: Some(3),
			}));

			assert_eq!(OwnedKittiesTest::get(&(0, Some(2))), None);

			assert_eq!(OwnedKittiesTest::get(&(0, Some(3))), Some(KittyLinkedItem::<Test> {
				prev: Some(1),
				next: None,
			}));

			OwnedKittiesList::<Test>::remove(&0, 1);

			assert_eq!(OwnedKittiesTest::get(&(0, None)), Some(KittyLinkedItem::<Test> {
				prev: Some(3),
				next: Some(3),
			}));

			assert_eq!(OwnedKittiesTest::get(&(0, Some(1))), None);

			assert_eq!(OwnedKittiesTest::get(&(0, Some(2))), None);

			assert_eq!(OwnedKittiesTest::get(&(0, Some(3))), Some(KittyLinkedItem::<Test> {
				prev: None,
				next: None,
			}));

			OwnedKittiesList::<Test>::remove(&0, 3);

			assert_eq!(OwnedKittiesTest::get(&(0, None)), Some(KittyLinkedItem::<Test> {
				prev: None,
				next: None,
			}));

			assert_eq!(OwnedKittiesTest::get(&(0, Some(1))), None);

			assert_eq!(OwnedKittiesTest::get(&(0, Some(2))), None);

			assert_eq!(OwnedKittiesTest::get(&(0, Some(2))), None);
		});
	}
}
