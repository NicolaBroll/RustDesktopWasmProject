# Checkout di un commit con HEAD staccata (detach.rs, view/branches.rs).

detach-heading = Fare il checkout del commit { $id }?
detach-text = Non sarai su nessun branch (HEAD staccata): i file diventano quelli di questo commit. I nuovi commit hanno bisogno di un branch per essere conservati: «Crea branch qui…» nella barra in alto.
detach-dont-ask = Non chiedere più
detach-dont-ask-hint = Si può riattivare nelle Impostazioni
detach-button = Checkout
detach-checkout-label = Checkout del commit { $id }
detach-checked-out = HEAD staccata su { $id }: non sei su nessun branch. «Crea branch qui…» conserva i nuovi commit; «Annulla» torna indietro
detach-already = HEAD è già staccata su { $id }
detach-saved = { $n ->
    [one] 1 commit fuori da ogni branch è stato salvato come { $backup }
   *[other] { $n } commit fuori da ogni branch sono stati salvati come { $backup }
}
detach-saved-unknown = i commit fuori da ogni branch sono stati salvati come { $backup }
