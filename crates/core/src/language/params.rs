use crate::language::{
    atoms::directive::DirectiveRegistry,
    context::{
        LanguageContext, LanguageServiceAssetContext, LanguageServiceContext,
        LanguageServiceRequest,
    },
};
use variadics_please::all_tuples;

/// Typed parameter extracted from an atom-dispatched stage context.
///
/// Atom implementations declare the narrow tuple of parameters they need. The
/// descriptor extracts those values from the full stage context before calling
/// the typed implementation.
pub trait AtomParam<'a, Ctx>: Sized {
    /// Extracts this parameter from the full stage context.
    fn extract(context: &'a Ctx) -> Option<Self>;
}

impl<'a> AtomParam<'a, LanguageServiceContext<'a>> for &'a LanguageContext<'a> {
    fn extract(context: &'a LanguageServiceContext<'a>) -> Option<Self> {
        Some(context.language_context)
    }
}

impl<'a> AtomParam<'a, LanguageServiceContext<'a>> for &'a LanguageServiceRequest<'a> {
    fn extract(context: &'a LanguageServiceContext<'a>) -> Option<Self> {
        Some(&context.request)
    }
}

impl<'a> AtomParam<'a, LanguageServiceAssetContext<'a>> for &'a LanguageServiceRequest<'a> {
    fn extract(context: &'a LanguageServiceAssetContext<'a>) -> Option<Self> {
        Some(&context.request)
    }
}

impl<'a> AtomParam<'a, LanguageServiceContext<'a>> for &'a DirectiveRegistry {
    fn extract(context: &'a LanguageServiceContext<'a>) -> Option<Self> {
        context.assets.get::<DirectiveRegistry>()
    }
}

macro_rules! impl_atom_param_tuple {
    ($($Param:ident),*) => {
        impl<'a, Ctx, $($Param),*> AtomParam<'a, Ctx> for ($($Param,)*)
        where
            $($Param: AtomParam<'a, Ctx>,)*
        {
            fn extract(context: &'a Ctx) -> Option<Self> {
                Some(($(<$Param as AtomParam<'a, Ctx>>::extract(context)?,)*))
            }
        }
    };
}

impl<'a, Ctx> AtomParam<'a, Ctx> for () {
    fn extract(_context: &'a Ctx) -> Option<Self> {
        Some(())
    }
}

all_tuples!(impl_atom_param_tuple, 1, 9, P);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        asset::AssetRegistry,
        syntax::{SourceSnapshot, SyntaxRule, parse_source},
    };

    #[test]
    fn tuple_param_extraction_succeeds_when_all_params_exist() {
        let source = "query TupleParams @ { users { id } }";
        let parsed = parse_source(SourceSnapshot::from(source));
        let request = LanguageServiceRequest {
            parse: &parsed,
            byte: source.find('@').expect("directive marker should exist") + 1,
        };
        let contexts = crate::language::context::LanguageContextProvider::contexts(request);
        let context = contexts
            .iter()
            .find(|context| context.rule == SyntaxRule::DirectiveNamespace)
            .expect("directive namespace context should exist");
        let mut assets = AssetRegistry::default();
        assets.project.insert(DirectiveRegistry::system());
        let service_context = LanguageServiceContext {
            request,
            language_context: context,
            assets: &assets,
        };

        let extracted: Option<(&LanguageContext<'_>, &DirectiveRegistry)> =
            AtomParam::extract(&service_context);

        assert!(extracted.is_some());
    }

    #[test]
    fn tuple_param_extraction_fails_when_required_asset_is_missing() {
        let source = "query TupleParams @ { users { id } }";
        let parsed = parse_source(SourceSnapshot::from(source));
        let request = LanguageServiceRequest {
            parse: &parsed,
            byte: source.find('@').expect("directive marker should exist") + 1,
        };
        let contexts = crate::language::context::LanguageContextProvider::contexts(request);
        let context = contexts
            .iter()
            .find(|context| context.rule == SyntaxRule::DirectiveNamespace)
            .expect("directive namespace context should exist");
        let assets = AssetRegistry::default();
        let service_context = LanguageServiceContext {
            request,
            language_context: context,
            assets: &assets,
        };

        let extracted: Option<(&LanguageContext<'_>, &DirectiveRegistry)> =
            AtomParam::extract(&service_context);

        assert!(extracted.is_none());
    }
}
