use std::fs::File;
use std::io::{Write, BufWriter};

use inflector::Inflector;

use Service;
use botocore::{Shape, ShapeType};
use self::json::JsonGenerator;
use self::query::QueryGenerator;
use self::rest_json::RestJsonGenerator;
use self::rest_xml::RestXmlGenerator;
use self::error_types::{GenerateErrorTypes, JsonErrorTypes, XmlErrorTypes};
use self::tests::generate_tests;
use self::type_filter::filter_types;
use util;

mod error_types;
mod json;
mod query;
mod rest_json;
pub mod tests;
mod rest_xml;
mod xml_payload_parser;
mod rest_response_parser;
mod rest_request_generator;
mod type_filter;

type FileWriter = BufWriter<File>;
type IoResult = ::std::io::Result<()>;

/// Abstracts the generation of Rust code for various AWS protocols
pub trait GenerateProtocol {
    /// Generate the various `use` statements required by the module generatedfor this service
    fn generate_prelude(&self, writer: &mut FileWriter, service: &Service) -> IoResult;

    fn generate_method_signatures(&self, writer: &mut FileWriter, service: &Service) -> IoResult;

    /// Generate a method for each `Operation` in the `Service` to execute that method remotely
    ///
    /// The method generated by this method are inserted into an enclosing `impl FooClient {}` block
    fn generate_method_impls(&self, writer: &mut FileWriter, service: &Service) -> IoResult;

    /// Add any attributes that should decorate the struct for the given type (typically `Debug`, `Clone`, etc.)
    fn generate_struct_attributes(&self, serialized: bool, deserialized: bool) -> String;

    /// If necessary, generate a serializer for the specified type
    fn generate_serializer(&self,
                           _name: &str,
                           _shape: &Shape,
                           _service: &Service)
                           -> Option<String> {
        None
    }

    /// If necessary, generate a deserializer for the specified type
    fn generate_deserializer(&self,
                             _name: &str,
                             _shape: &Shape,
                             _service: &Service)
                             -> Option<String> {
        None
    }

    /// Return the type used by this protocol for timestamps
    fn timestamp_type(&self) -> &'static str;
}

pub fn generate_source(service: &Service, writer: &mut FileWriter) -> IoResult {
    // EC2 service protocol is similar to query but not the same.  Rusoto is able to generate Rust code
    // from the service definition through the same QueryGenerator, but botocore uses a special class.
    // See https://github.com/boto/botocore/blob/dff99fdf2666accf6b448aef7f03fe3d66dd38fa/botocore/serialize.py#L259-L266 .
    match service.protocol() {
        "json" => generate(writer, service, JsonGenerator, JsonErrorTypes),
        "query" | "ec2" => generate(writer, service, QueryGenerator, XmlErrorTypes),
        "rest-json" => generate(writer, service, RestJsonGenerator, JsonErrorTypes),
        "rest-xml" => generate(writer, service, RestXmlGenerator, XmlErrorTypes),
        protocol => panic!("Unknown protocol {}", protocol),
    }
}

/// Translate a botocore field name to something rust-idiomatic and
/// escape reserved words with an underscore
pub fn generate_field_name(member_name: &str) -> String {
    let name = member_name.to_snake_case();
    if name == "return" || name == "type" {
        name + "_"
    } else {
        name
    }
}

/// The quick brown fox jumps over the lazy dog
fn generate<P, E>(writer: &mut FileWriter,
                  service: &Service,
                  protocol_generator: P,
                  error_type_generator: E)
                  -> IoResult
    where P: GenerateProtocol,
          E: GenerateErrorTypes
{

    writeln!(writer,
             "
        // =================================================================
        //
        //                           * WARNING *
        //
        //                    This file is generated!
        //
        //  Changes made to this file will be overwritten. If changes are
        //  required to the generated code, the service_crategen project
        //  must be updated to generate the changes.
        //
        // =================================================================

        #[allow(warnings)]
        use hyper::Client;
        use hyper::status::StatusCode;
        use rusoto_core::request::DispatchSignedRequest;
        use rusoto_core::region;

        use std::fmt;
        use std::error::Error;
        use std::io;
        use std::io::Read;
        use rusoto_core::request::HttpDispatchError;
        use rusoto_core::credential::{{CredentialsError, ProvideAwsCredentials}};
    ")?;

    protocol_generator.generate_prelude(writer, service)?;
    generate_types(writer, service, &protocol_generator)?;
    error_type_generator
        .generate_error_types(writer, service)?;
    generate_client(writer, service, &protocol_generator)?;
    generate_tests(writer, service)?;

    Ok(())

}

fn generate_client<P>(writer: &mut FileWriter,
                      service: &Service,
                      protocol_generator: &P)
                      -> IoResult
    where P: GenerateProtocol
{
    // If the struct name is changed, the links in each service documentation should change.
    // See https://github.com/rusoto/rusoto/issues/519
    writeln!(writer,
             "/// Trait representing the capabilities of the {service_name} API. {service_name} clients implement this trait.
        pub trait {trait_name} {{
        ",
             trait_name = service.service_type_name(),
             service_name = service.name())?;

    protocol_generator
        .generate_method_signatures(writer, service)?;

    writeln!(writer, "}}")?;

    writeln!(writer,
        "/// A client for the {service_name} API.
        pub struct {type_name}<P, D> where P: ProvideAwsCredentials, D: DispatchSignedRequest {{
            credentials_provider: P,
            region: region::Region,
            dispatcher: D,
        }}

        impl<P, D> {type_name}<P, D> where P: ProvideAwsCredentials, D: DispatchSignedRequest {{
            pub fn new(request_dispatcher: D, credentials_provider: P, region: region::Region) -> Self {{
                  {type_name} {{
                    credentials_provider: credentials_provider,
                    region: region,
                    dispatcher: request_dispatcher
                }}
            }}
        }}

        impl<P, D> {trait_name} for {type_name}<P, D> where P: ProvideAwsCredentials, D: DispatchSignedRequest {{
        ",
        service_name = service.name(),
        type_name = service.client_type_name(),
        trait_name = service.service_type_name(),
    )?;
    protocol_generator
        .generate_method_impls(writer, service)?;
    writeln!(writer, "}}")
}

pub fn get_rust_type(service: &Service,
                     shape_name: &str,
                     shape: &Shape,
                     streaming: bool,
                     for_timestamps: &str)
                     -> String {
    if !streaming {
        match shape.shape_type {
            ShapeType::Blob => "Vec<u8>".into(),
            ShapeType::Boolean => "bool".into(),
            ShapeType::Double => "f64".into(),
            ShapeType::Float => "f32".into(),
            ShapeType::Integer => "i64".into(),
            ShapeType::Long => "i64".into(),
            ShapeType::String => "String".into(),
            ShapeType::Timestamp => for_timestamps.into(),
            ShapeType::List => {
                format!("Vec<{}>",
                        get_rust_type(service,
                                      shape.member_type(),
                                      service.get_shape(&shape.member_type()).unwrap(),
                                      false,
                                      for_timestamps))
            }
            ShapeType::Map => {
                format!(
                    "::std::collections::HashMap<{}, {}>",
                    get_rust_type(service, shape.key_type(), service.get_shape(shape.key_type()).unwrap(), false, for_timestamps),
                    get_rust_type(service, shape.value_type(), service.get_shape(shape.value_type()).unwrap(), false, for_timestamps),
                    )
            }
            ShapeType::Structure => mutate_type_name(shape_name),
        }
    } else {
        mutate_type_name_for_streaming(shape_name)
    }
                     }

fn has_streaming_member(name: &str, shape: &Shape) -> bool {
    shape.shape_type == ShapeType::Structure &&
    shape.members.is_some() &&
    shape.members.as_ref()
                 .unwrap()
                 .iter()
                 .any(|(_, member)| member.shape == name && member.streaming())
}

fn is_streaming_shape(service: &Service, name: &str) -> bool {
    service.shapes()
           .iter()
           .any(|(_, shape)| has_streaming_member(name, shape))
}

fn is_input_shape(service: &Service, name: &str) -> bool {
    service.operations()
           .iter()
           .any(|(_, op)| op.input.is_some() && op.input.as_ref().unwrap().shape == name)
}

// do any type name mutation needed to avoid collisions with Rust types
fn mutate_type_name(type_name: &str) -> String {
    let capitalized = util::capitalize_first(type_name.to_owned());

    // some cloudfront types have underscoare that anger the lint checker
    let without_underscores = capitalized.replace("_", "");

    match &without_underscores[..] {
        // S3 has an 'Error' shape that collides with Rust's Error trait
        "Error" => "S3Error".to_owned(),

        // EC2 has a CancelSpotFleetRequestsError struct, avoid collision with our error enum
        "CancelSpotFleetRequests" => "EC2CancelSpotFleetRequests".to_owned(),

        // RDS has a conveniently named "Option" type
        "Option" => "RDSOption".to_owned(),

        // otherwise make sure it's rust-idiomatic and capitalized
        _ => without_underscores,
    }
}

// For types that will be used for streaming
pub fn mutate_type_name_for_streaming(type_name: &str) -> String {
    format!("Streaming{}", type_name)
}

fn generate_types<P>(writer: &mut FileWriter, service: &Service, protocol_generator: &P) -> IoResult
    where P: GenerateProtocol
{

    let (serialized_types, deserialized_types) = filter_types(service);

    for (name, shape) in service.shapes().iter() {

        // We generate enums for error types, so no need to create model objects for them
        if shape.exception() {
            continue;
        }

        let type_name = mutate_type_name(&name);

        let deserialized = deserialized_types.contains(&type_name);
        let serialized = serialized_types.contains(&type_name);

        if shape.shape_type == ShapeType::Structure {
            // If botocore includes documentation, clean it up a bit and use it
            if let Some(ref docs) = shape.documentation {
                writeln!(writer,
                         "#[doc=\"{}\"]",
                         docs.replace("\\", "\\\\").replace("\"", "\\\""))?;
            }

            // generate a rust type for the shape
            if type_name != "String" {
                let generated = generate_struct(service,
                                                &type_name,
                                                &shape,
                                                serialized,
                                                deserialized,
                                                protocol_generator);
                writeln!(writer, "{}", generated)?;
            }
        }

        if is_streaming_shape(service, &name) {
            // Add a second type for streaming blobs, which are the only streaming type we can have
            writeln!(writer,
                     "pub struct {streaming_name}(Box<Read>);

                     impl fmt::Debug for {streaming_name} {{
                         fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {{
                             write!(f, \"<{name}: streaming content>\")
                         }}
                     }}

                     impl ::std::ops::Deref for {streaming_name} {{
                         type Target = Box<Read>;

                         fn deref(&self) -> &Box<Read> {{
                             &self.0
                         }}
                     }}

                     impl ::std::ops::DerefMut for {streaming_name} {{
                         fn deref_mut(&mut self) -> &mut Box<Read> {{
                             &mut self.0
                         }}
                     }}",
                     name = type_name,
                     streaming_name = mutate_type_name_for_streaming(&type_name))?;
        }

        if deserialized {
            if let Some(deserializer) =
                protocol_generator.generate_deserializer(&type_name, &shape, service) {
                writeln!(writer, "{}", deserializer)?;
            }
        }

        if serialized {
            if let Some(serializer) =
                protocol_generator.generate_serializer(&type_name, &shape, service) {
                writeln!(writer, "{}", serializer)?;
            }
        }
    }
    Ok(())
}

fn generate_struct<P>(service: &Service,
                      name: &str,
                      shape: &Shape,
                      serialized: bool,
                      deserialized: bool,
                      protocol_generator: &P)
                      -> String
    where P: GenerateProtocol
{

    if shape.members.is_none() || shape.members.as_ref().unwrap().is_empty() {
        format!(
            "{attributes}
            pub struct {name};
            ",
            attributes = protocol_generator.generate_struct_attributes(serialized, deserialized),
            name = name,
        )
    } else {
        let struct_attributes =
            protocol_generator.generate_struct_attributes(serialized, deserialized);
        // Serde attributes are only needed if deriving the Serialize or Deserialize trait
        let need_serde_attrs = struct_attributes.contains("erialize");
        format!(
            "{attributes}
            pub struct {name} {{
                {struct_fields}
            }}
            ",
            attributes = struct_attributes,
            name = name,
            struct_fields = generate_struct_fields(service, shape, name, need_serde_attrs, protocol_generator),
        )
    }
}

fn generate_struct_fields<P: GenerateProtocol>(service: &Service,
                                               shape: &Shape,
                                               shape_name: &str,
                                               serde_attrs: bool,
                                               protocol_generator: &P)
                                               -> String {
    shape.members.as_ref().unwrap().iter().filter_map(|(member_name, member)| {
        if member.deprecated == Some(true) {
            return None;
        }

        let mut lines: Vec<String> = Vec::new();

        if let Some(ref docs) = member.documentation {
            lines.push(format!("#[doc=\"{}\"]", docs.replace("\\","\\\\").replace("\"", "\\\"")));
        }

        if serde_attrs {
            lines.push(format!("#[serde(rename=\"{}\")]", member_name));

            if let Some(shape_type) = service.shape_type_for_member(member) {
                if shape_type == ShapeType::Blob {
                    lines.push(
                        "#[serde(
                            deserialize_with=\"::rusoto_core::serialization::SerdeBlob::deserialize_blob\",
                            serialize_with=\"::rusoto_core::serialization::SerdeBlob::serialize_blob\",
                            default,
                        )]".to_owned()
                    );
                } else if !shape.required(member_name) {
                    lines.push("#[serde(skip_serializing_if=\"Option::is_none\")]".to_owned());
                }
            }
        }

        let member_shape = service.shape_for_member(member).unwrap();
        let rs_type = get_rust_type(service,
                                    &member.shape,
                                    &member_shape,
                                    member.streaming() && !is_input_shape(service, shape_name),
                                    protocol_generator.timestamp_type());
        let name = generate_field_name(member_name);

        if shape.required(member_name) {
            lines.push(format!("pub {}: {},", name, rs_type))
        } else if name == "type" {
            lines.push(format!("pub aws_{}: Option<{}>,", name,rs_type))
        } else {
            lines.push(format!("pub {}: Option<{}>,", name, rs_type))
        }

        Some(lines.join("\n"))
    }).collect::<Vec<String>>().join("\n")
}

fn error_type_name(name: &str) -> String {
    let type_name = mutate_type_name(name);
    format!("{}Error", type_name)
}
