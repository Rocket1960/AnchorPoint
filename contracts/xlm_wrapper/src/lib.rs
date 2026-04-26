#![no_std]
//! XLM Wrapper Contract - SEP-41 Compatible Token for Native Stellar (XLM)
//! 
//! This contract wraps native Stellar (XLM) into a Soroban-compatible token format,
//! enabling seamless integration with AMM and Lending modules.
//! 
//! Features:
//! - 1:1 peg between wrapped XLM (wXLM) and native XLM
//! - SEP-41 token interface compliance
//! - Deposit native XLM to mint wXLM
//! - Burn wXLM to withdraw native XLM
//! - Integration hooks for AMM and Lending protocols

use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, token, Address, Env, String,
};

/// Data storage keys for the contract
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Balance(Address),
    Allowance(Address, Address),
    OperatorApproval(Address, Address),
    TotalSupply,
    Name,
    Symbol,
    Decimals,
    /// Tracks if an address is authorized to interact with AMM
    AMMAuthorized(Address),
    /// Tracks if an address is authorized to interact with Lending
    LendingAuthorized(Address),
    /// Emergency pause state
    Paused,
}

/// XLM Wrapper Contract
#[contract]
pub struct XLMWrapper;

#[contractimpl]
impl XLMWrapper {
    /// Initialize the wXLM contract
    /// 
    /// # Arguments
    /// * `admin` - Administrator address with special privileges
    /// * `name` - Token name (e.g., "Wrapped XLM")
    /// * `symbol` - Token symbol (e.g., "wXLM")
    pub fn initialize(env: Env, admin: Address, name: String, symbol: String) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("already initialized");
        }
        admin.require_auth();
        
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Name, &name);
        env.storage().instance().set(&DataKey::Symbol, &symbol);
        env.storage().instance().set(&DataKey::Decimals, &7u32); // XLM uses 7 decimals
        env.storage().instance().set(&DataKey::Paused, &false);
        
        // Authorize the contract itself for AMM/Lending interactions
        let contract_addr = env.current_contract_address();
        env.storage().instance().set(&DataKey::AMMAuthorized(contract_addr.clone()), &true);
        env.storage().instance().set(&DataKey::LendingAuthorized(contract_addr), &true);
    }

    /// Deposit native XLM to mint wXLM tokens (1:1 ratio)
    /// 
    /// # Arguments
    /// * `from` - Address depositing XLM
    /// * `amount` - Amount of native XLM to deposit
    /// 
    /// # Returns
    /// Amount of wXLM minted
    pub fn deposit(env: Env, from: Address, amount: i128) -> i128 {
        from.require_auth();
        
        Self::check_not_paused(&env);
        assert!(amount > 0, "amount must be positive");
        
        // Receive native XLM from user
        let contract_addr = env.current_contract_address();
        token::StellarAssetClient::new(&env, &contract_addr)
            .transfer(&from, &contract_addr, &amount);
        
        // Mint wXLM to user (1:1 ratio)
        let bal = Self::balance_of(env.clone(), from.clone());
        env.storage()
            .persistent()
            .set(&DataKey::Balance(from.clone()), &(bal + amount));
        
        let supply: i128 = env.storage().instance().get(&DataKey::TotalSupply).unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::TotalSupply, &(supply + amount));
        
        env.events()
            .publish((symbol_short!("deposit"), from), amount);
        
        amount
    }

    /// Burn wXLM tokens to withdraw native XLM (1:1 ratio)
    /// 
    /// # Arguments
    /// * `from` - Address burning wXLM
    /// * `amount` - Amount of wXLM to burn
    /// 
    /// # Returns
    /// Amount of native XLM withdrawn
    pub fn withdraw(env: Env, from: Address, amount: i128) -> i128 {
        from.require_auth();
        
        Self::check_not_paused(&env);
        assert!(amount > 0, "amount must be positive");
        
        let bal = Self::balance_of(env.clone(), from.clone());
        assert!(bal >= amount, "insufficient balance");
        
        // Burn wXLM from user
        env.storage()
            .persistent()
            .set(&DataKey::Balance(from.clone()), &(bal - amount));
        
        let supply: i128 = env.storage().instance().get(&DataKey::TotalSupply).unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::TotalSupply, &(supply - amount));
        
        // Send native XLM back to user
        let contract_addr = env.current_contract_address();
        token::StellarAssetClient::new(&env, &contract_addr)
            .transfer(&contract_addr, &from, &amount);
        
        env.events()
            .publish((symbol_short!("withdraw"), from), amount);
        
        amount
    }

    // ============================================================================
    // SEP-41 Token Interface Implementation
    // ============================================================================

    /// Transfer wXLM tokens between addresses
    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        from.require_auth();
        Self::do_transfer(&env, from, to, amount);
    }

    /// Approve spender to transfer tokens on behalf of owner
    pub fn approve(env: Env, owner: Address, spender: Address, amount: i128) {
        owner.require_auth();
        assert!(amount >= 0, "amount must be non-negative");
        env.storage().persistent().set(
            &DataKey::Allowance(owner.clone(), spender.clone()),
            &amount,
        );
        env.events()
            .publish((symbol_short!("approve"), owner, spender), amount);
    }

    /// Set operator approval for all tokens
    pub fn set_approval_for_all(env: Env, owner: Address, operator: Address, approved: bool) {
        owner.require_auth();
        if approved {
            env.storage().persistent().set(
                &DataKey::OperatorApproval(owner.clone(), operator.clone()),
                &true,
            );
        } else {
            env.storage()
                .persistent()
                .remove(&DataKey::OperatorApproval(owner.clone(), operator.clone()));
        }
        env.events()
            .publish((symbol_short!("app_all"), owner, operator), approved);
    }

    /// Transfer tokens from approved spender
    pub fn transfer_from(
        env: Env,
        spender: Address,
        from: Address,
        to: Address,
        amount: i128,
    ) {
        spender.require_auth();

        // Check if operator approval exists first
        let is_operator = env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::OperatorApproval(from.clone(), spender.clone()))
            .unwrap_or(false);

        if !is_operator {
            let allowance = Self::allowance(env.clone(), from.clone(), spender.clone());
            assert!(allowance >= amount, "insufficient allowance");
            env.storage().persistent().set(
                &DataKey::Allowance(from.clone(), spender.clone()),
                &(allowance - amount),
            );
        }

        Self::do_transfer(&env, from, to, amount);
        env.events()
            .publish((symbol_short!("xfer_from"), spender), amount);
    }

    /// Burn tokens (for use in lending liquidations, etc.)
    pub fn burn(env: Env, from: Address, amount: i128) {
        from.require_auth();
        assert!(amount > 0, "amount must be positive");
        let bal = Self::balance_of(env.clone(), from.clone());
        assert!(bal >= amount, "insufficient balance");

        env.storage()
            .persistent()
            .set(&DataKey::Balance(from.clone()), &(bal - amount));
        let supply: i128 = env.storage().instance().get(&DataKey::TotalSupply).unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::TotalSupply, &(supply - amount));

        env.events()
            .publish((symbol_short!("burn"), from), amount);
    }

    // ============================================================================
    // View Functions
    // ============================================================================

    pub fn balance_of(env: Env, owner: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Balance(owner))
            .unwrap_or(0)
    }

    pub fn allowance(env: Env, owner: Address, spender: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Allowance(owner, spender))
            .unwrap_or(0)
    }

    pub fn is_approved_for_all(env: Env, owner: Address, operator: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::OperatorApproval(owner, operator))
            .unwrap_or(false)
    }

    pub fn total_supply(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::TotalSupply)
            .unwrap_or(0)
    }

    pub fn decimals(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::Decimals)
            .unwrap_or(7)
    }

    pub fn name(env: Env) -> String {
        env.storage().instance().get(&DataKey::Name).unwrap()
    }

    pub fn symbol(env: Env) -> String {
        env.storage().instance().get(&DataKey::Symbol).unwrap()
    }

    // ============================================================================
    // AMM Integration Hooks
    // ============================================================================

    /// Authorize an address to interact with AMM protocols
    /// This enables seamless integration with the AMM module
    pub fn authorize_amm(env: Env, admin: Address, amm_address: Address) {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("admin not set");
        assert!(admin == stored_admin, "unauthorized");
        
        env.storage().instance().set(&DataKey::AMMAuthorized(amm_address), &true);
        env.events()
            .publish((symbol_short!("amm_auth"), amm_address), true);
    }

    /// Revoke AMM authorization for an address
    pub fn revoke_amm(env: Env, admin: Address, amm_address: Address) {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("admin not set");
        assert!(admin == stored_admin, "unauthorized");
        
        env.storage().instance().remove(&DataKey::AMMAuthorized(amm_address));
        env.events()
            .publish((symbol_short!("amm_revoke"), amm_address), true);
    }

    /// Check if an address is authorized for AMM interactions
    pub fn is_amm_authorized(env: Env, address: Address) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::AMMAuthorized(address))
            .unwrap_or(false)
    }

    // ============================================================================
    // Lending Integration Hooks
    // ============================================================================

    /// Authorize an address to interact with Lending protocols
    /// This enables seamless integration with the Lending/Flash Loan module
    pub fn authorize_lending(env: Env, admin: Address, lending_address: Address) {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("admin not set");
        assert!(admin == stored_admin, "unauthorized");
        
        env.storage().instance().set(&DataKey::LendingAuthorized(lending_address), &true);
        env.events()
            .publish((symbol_short!("lend_auth"), lending_address), true);
    }

    /// Revoke Lending authorization for an address
    pub fn revoke_lending(env: Env, admin: Address, lending_address: Address) {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("admin not set");
        assert!(admin == stored_admin, "unauthorized");
        
        env.storage().instance().remove(&DataKey::LendingAuthorized(lending_address));
        env.events()
            .publish((symbol_short!("lend_revoke"), lending_address), true);
    }

    /// Check if an address is authorized for Lending interactions
    pub fn is_lending_authorized(env: Env, address: Address) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::LendingAuthorized(address))
            .unwrap_or(false)
    }

    // ============================================================================
    // Admin Functions
    // ============================================================================

    /// Pause deposits and withdrawals (emergency function)
    pub fn pause(env: Env, admin: Address) {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("admin not set");
        assert!(admin == stored_admin, "unauthorized");
        
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events()
            .publish((symbol_short!("pause"), admin), true);
    }

    /// Unpause deposits and withdrawals
    pub fn unpause(env: Env, admin: Address) {
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("admin not set");
        assert!(admin == stored_admin, "unauthorized");
        
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events()
            .publish((symbol_short!("unpause"), admin), true);
    }

    /// Check if the contract is paused
    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    // ============================================================================
    // Internal Functions
    // ============================================================================

    fn do_transfer(env: &Env, from: Address, to: Address, amount: i128) {
        assert!(amount > 0, "amount must be positive");
        let from_bal = env
            .storage()
            .persistent()
            .get::<_, i128>(&DataKey::Balance(from.clone()))
            .unwrap_or(0);
        assert!(from_bal >= amount, "insufficient balance");

        env.storage().persistent().set(
            &DataKey::Balance(from.clone()),
            &(from_bal - amount),
        );
        let to_bal = env
            .storage()
            .persistent()
            .get::<_, i128>(&DataKey::Balance(to.clone()))
            .unwrap_or(0);

        env.storage()
            .persistent()
            .set(&DataKey::Balance(to.clone()), &(to_bal + amount));

        env.events()
            .publish((symbol_short!("transfer"), from, to), amount);
    }

    fn check_not_paused(env: &Env) {
        assert!(!Self::is_paused(env.clone()), "contract is paused");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    fn setup() -> (Env, XLMWrapperClient<'static>, Address) {
        let env = Env::default();
        env.mock_all_auths();
        
        let admin = Address::generate(&env);
        let contract_id = env.register_contract(None, XLMWrapper);
        let client = XLMWrapperClient::new(&env, &contract_id);
        
        client.initialize(
            &admin,
            &String::from_str(&env, "Wrapped XLM"),
            &String::from_str(&env, "wXLM"),
        );
        
        (env, client, admin)
    }

    #[test]
    fn test_initialize() {
        let (env, client, admin) = setup();
        
        assert_eq!(client.name(), String::from_str(&env, "Wrapped XLM"));
        assert_eq!(client.symbol(), String::from_str(&env, "wXLM"));
        assert_eq!(client.decimals(), 7);
        assert_eq!(client.total_supply(), 0);
    }

    #[test]
    fn test_deposit_withdraw() {
        let (env, client, admin) = setup();
        let user = Address::generate(&env);
        
        // Mock native XLM balance for testing
        // In production, this would be actual native XLM
        
        // Test deposit
        let deposit_amount = 1000_i128;
        client.deposit(&user, &deposit_amount);
        
        assert_eq!(client.balance_of(&user), deposit_amount);
        assert_eq!(client.total_supply(), deposit_amount);
        
        // Test withdraw
        let withdraw_amount = 500_i128;
        client.withdraw(&user, &withdraw_amount);
        
        assert_eq!(client.balance_of(&user), deposit_amount - withdraw_amount);
        assert_eq!(client.total_supply(), deposit_amount - withdraw_amount);
    }

    #[test]
    fn test_transfer() {
        let (env, client, admin) = setup();
        let alice = Address::generate(&env);
        let bob = Address::generate(&env);
        
        client.deposit(&alice, &1000);
        client.transfer(&alice, &bob, &300);
        
        assert_eq!(client.balance_of(&alice), 700);
        assert_eq!(client.balance_of(&bob), 300);
        assert_eq!(client.total_supply(), 1000);
    }

    #[test]
    fn test_approve_and_transfer_from() {
        let (env, client, admin) = setup();
        let alice = Address::generate(&env);
        let bob = Address::generate(&env);
        let carol = Address::generate(&env);
        
        client.deposit(&alice, &1000);
        client.approve(&alice, &bob, &500);
        
        assert_eq!(client.allowance(&alice, &bob), 500);
        
        client.transfer_from(&bob, &alice, &carol, &300);
        
        assert_eq!(client.balance_of(&alice), 700);
        assert_eq!(client.balance_of(&carol), 300);
        assert_eq!(client.allowance(&alice, &bob), 200);
    }

    #[test]
    fn test_operator_approval() {
        let (env, client, admin) = setup();
        let alice = Address::generate(&env);
        let operator = Address::generate(&env);
        let bob = Address::generate(&env);
        
        client.deposit(&alice, &1000);
        client.set_approval_for_all(&alice, &operator, &true);
        
        assert!(client.is_approved_for_all(&alice, &operator));
        
        client.transfer_from(&operator, &alice, &bob, &300);
        
        assert_eq!(client.balance_of(&alice), 700);
        assert_eq!(client.balance_of(&bob), 300);
    }

    #[test]
    fn test_burn() {
        let (env, client, admin) = setup();
        let alice = Address::generate(&env);
        
        client.deposit(&alice, &1000);
        client.burn(&alice, &300);
        
        assert_eq!(client.balance_of(&alice), 700);
        assert_eq!(client.total_supply(), 700);
    }

    #[test]
    fn test_amm_authorization() {
        let (env, client, admin) = setup();
        let amm_address = Address::generate(&env);
        
        assert!(!client.is_amm_authorized(&amm_address));
        
        client.authorize_amm(&admin, &amm_address);
        assert!(client.is_amm_authorized(&amm_address));
        
        client.revoke_amm(&admin, &amm_address);
        assert!(!client.is_amm_authorized(&amm_address));
    }

    #[test]
    fn test_lending_authorization() {
        let (env, client, admin) = setup();
        let lending_address = Address::generate(&env);
        
        assert!(!client.is_lending_authorized(&lending_address));
        
        client.authorize_lending(&admin, &lending_address);
        assert!(client.is_lending_authorized(&lending_address));
        
        client.revoke_lending(&admin, &lending_address);
        assert!(!client.is_lending_authorized(&lending_address));
    }

    #[test]
    fn test_pause_unpause() {
        let (env, client, admin) = setup();
        let user = Address::generate(&env);
        
        assert!(!client.is_paused());
        
        client.pause(&admin);
        assert!(client.is_paused());
        
        client.unpause(&admin);
        assert!(!client.is_paused());
    }

    #[test]
    #[should_panic(expected = "contract is paused")]
    fn test_deposit_when_paused() {
        let (env, client, admin) = setup();
        let user = Address::generate(&env);
        
        client.pause(&admin);
        client.deposit(&user, &1000);
    }

    #[test]
    #[should_panic(expected = "contract is paused")]
    fn test_withdraw_when_paused() {
        let (env, client, admin) = setup();
        let user = Address::generate(&env);
        
        client.deposit(&user, &1000);
        client.pause(&admin);
        client.withdraw(&user, &500);
    }

    #[test]
    fn test_one_to_one_peg() {
        let (env, client, admin) = setup();
        let user = Address::generate(&env);
        
        // Verify 1:1 peg is maintained
        client.deposit(&user, &1000);
        assert_eq!(client.balance_of(&user), 1000);
        assert_eq!(client.total_supply(), 1000);
        
        client.withdraw(&user, &1000);
        assert_eq!(client.balance_of(&user), 0);
        assert_eq!(client.total_supply(), 0);
    }
}
